use crate::{copy, http_config::DEFAULT_CONFIG, Body, HttpConfig, MutCow};
use encoding_rs::Encoding;
use futures_lite::{ready, AsyncRead, AsyncReadExt, AsyncWrite, Stream};
use httparse::{InvalidChunkSize, Status};
use std::{
    fmt::{self, Formatter},
    future::{Future, IntoFuture},
    io::{self, ErrorKind},
    iter,
    pin::Pin,
    task::{Context, Poll},
};
use Poll::{Pending, Ready};
use ReceivedBodyState::{Chunked, End, FixedLength, Start};

#[cfg(test)]
mod tests;

macro_rules! trace {
    ($s:literal, $($arg:tt)+) => (
        log::trace!(concat!(":{} ", $s), line!(), $($arg)+);
    )
}

/** A received http body

This type represents a body that will be read from the underlying
transport, which it may either borrow from a [`Conn`](crate::Conn) or
own.

```rust
# trillium_testing::block_on(async {
# use trillium_http::{Method, Conn};
let mut conn = Conn::new_synthetic(Method::Get, "/", "hello");
let body = conn.request_body().await;
assert_eq!(body.read_string().await?, "hello");
# trillium_http::Result::Ok(()) }).unwrap();
```

## Bounds checking

Every `ReceivedBody` has a maximum length beyond which it will return an error, expressed as a
u64. To override this on the specific `ReceivedBody`, use [`ReceivedBody::with_max_len`] or
[`ReceivedBody::set_max_len`]

The default maximum length is currently set to 500mb. In the next semver-minor release, this value
will decrease substantially.

## Large chunks, small read buffers

Attempting to read a chunked body with a buffer that is shorter than the chunk size in hex will
result in an error. This limitation is temporary.
*/

pub struct ReceivedBody<'conn, Transport> {
    content_length: Option<u64>,
    buffer: MutCow<'conn, Option<Vec<u8>>>,
    transport: Option<MutCow<'conn, Transport>>,
    state: MutCow<'conn, ReceivedBodyState>,
    on_completion: Option<Box<dyn Fn(Transport) + Send + Sync + 'static>>,
    encoding: &'static Encoding,
    max_len: u64,
    initial_len: usize,
    copy_loops_per_yield: usize,
}

fn slice_from(min: u64, buf: &[u8]) -> Option<&[u8]> {
    buf.get(usize::try_from(min).unwrap_or(usize::MAX)..)
        .filter(|buf| !buf.is_empty())
}

impl<'conn, Transport> ReceivedBody<'conn, Transport>
where
    Transport: AsyncRead + Unpin + Send + Sync + 'static,
{
    #[allow(missing_docs)]
    #[doc(hidden)]
    pub fn new(
        content_length: Option<u64>,
        buffer: impl Into<MutCow<'conn, Option<Vec<u8>>>>,
        transport: impl Into<MutCow<'conn, Transport>>,
        state: impl Into<MutCow<'conn, ReceivedBodyState>>,
        on_completion: Option<Box<dyn Fn(Transport) + Send + Sync + 'static>>,
        encoding: &'static Encoding,
    ) -> Self {
        Self::new_with_config(
            content_length,
            buffer,
            transport,
            state,
            on_completion,
            encoding,
            &DEFAULT_CONFIG,
        )
    }

    #[allow(missing_docs)]
    #[doc(hidden)]
    pub(crate) fn new_with_config(
        content_length: Option<u64>,
        buffer: impl Into<MutCow<'conn, Option<Vec<u8>>>>,
        transport: impl Into<MutCow<'conn, Transport>>,
        state: impl Into<MutCow<'conn, ReceivedBodyState>>,
        on_completion: Option<Box<dyn Fn(Transport) + Send + Sync + 'static>>,
        encoding: &'static Encoding,
        config: &HttpConfig,
    ) -> Self {
        Self {
            content_length,
            buffer: buffer.into(),
            transport: Some(transport.into()),
            state: state.into(),
            on_completion,
            encoding,
            max_len: config.received_body_max_len,
            initial_len: config.received_body_initial_len,
            copy_loops_per_yield: config.copy_loops_per_yield,
        }
    }

    /**
    Returns the content-length of this body, if available. This
    usually is derived from the content-length header. If the http
    request or response that this body is attached to uses
    transfer-encoding chunked, this will be None.

    ```rust
    # trillium_testing::block_on(async {
    # use trillium_http::{Method, Conn};
    let mut conn = Conn::new_synthetic(Method::Get, "/", "hello");
    let body = conn.request_body().await;
    assert_eq!(body.content_length(), Some(5));
    # trillium_http::Result::Ok(()) }).unwrap();
    ```
    */
    pub fn content_length(&self) -> Option<u64> {
        self.content_length
    }

    /// # Reads entire body to String.
    ///
    /// This uses the encoding determined by the content-type (mime)
    /// charset. If an encoding problem is encountered, the String
    /// returned by [`ReceivedBody::read_string`] will contain utf8
    /// replacement characters.
    ///
    /// Note that this can only be performed once per Conn, as the
    /// underlying data is not cached anywhere. This is the only copy of
    /// the body contents.
    ///
    /// # Errors
    ///
    /// This will return an error if there is an IO error on the
    /// underlying transport such as a disconnect
    ///
    /// This will also return an error if the length exceeds the maximum length. To override this
    /// value on this specific body, use [`ReceivedBody::with_max_len`] or
    /// [`ReceivedBody::set_max_len`]
    pub async fn read_string(self) -> crate::Result<String> {
        let encoding = self.encoding();
        let bytes = self.read_bytes().await?;
        let (s, _, _) = encoding.decode(&bytes);
        Ok(s.to_string())
    }

    fn owns_transport(&self) -> bool {
        self.transport
            .as_ref()
            .map(MutCow::is_owned)
            .unwrap_or_default()
    }

    /// Set the maximum length that can be read from this body before error
    pub fn set_max_len(&mut self, max_len: u64) {
        self.max_len = max_len;
    }

    /// chainable setter for the maximum length that can be read from this body before error
    #[must_use]
    pub fn with_max_len(mut self, max_len: u64) -> Self {
        self.set_max_len(max_len);
        self
    }

    /// Similar to [`ReceivedBody::read_string`], but returns the raw bytes. This is useful for
    /// bodies that are not text.
    ///
    /// You can use this in conjunction with `encoding` if you need different handling of malformed
    /// character encoding than the lossy conversion provided by [`ReceivedBody::read_string`].
    ///
    /// # Errors
    ///
    /// This will return an error if there is an IO error on the underlying transport such as a
    /// disconnect
    ///
    /// This will also return an error if the length exceeds
    /// [`received_body_max_len`][HttpConfig::with_received_body_max_len]. To override this value on
    /// this specific body, use [`ReceivedBody::with_max_len`] or [`ReceivedBody::set_max_len`]
    pub async fn read_bytes(mut self) -> crate::Result<Vec<u8>> {
        let mut vec = if let Some(len) = self.content_length {
            if len > self.max_len {
                return Err(crate::Error::ReceivedBodyTooLong(self.max_len));
            }

            let len = usize::try_from(len)
                .map_err(|_| crate::Error::ReceivedBodyTooLong(self.max_len))?;

            Vec::with_capacity(len)
        } else {
            Vec::with_capacity(self.initial_len)
        };

        self.read_to_end(&mut vec).await?;
        Ok(vec)
    }

    /**
    returns the character encoding of this body, usually determined from the content type
    (mime-type) of the associated Conn.
    */
    pub fn encoding(&self) -> &'static Encoding {
        self.encoding
    }

    fn read_raw(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        if let Some(transport) = self.transport.as_deref_mut() {
            read_raw(&mut self.buffer, transport, cx, buf)
        } else {
            Ready(Err(ErrorKind::NotConnected.into()))
        }
    }

    /**
    Consumes the remainder of this body from the underlying transport by reading it to the end and
    discarding the contents. This is important for http1.1 keepalive, but most of the time you do
    not need to directly call this. It returns the number of bytes consumed.

    # Errors

    This will return an [`std::io::Result::Err`] if there is an io error on the underlying
    transport, such as a disconnect
    */
    #[allow(clippy::missing_errors_doc)] // false positive
    pub async fn drain(self) -> io::Result<u64> {
        let copy_loops_per_yield = self.copy_loops_per_yield;
        copy(self, futures_lite::io::sink(), copy_loops_per_yield).await
    }
}

impl<'a, Transport> IntoFuture for ReceivedBody<'a, Transport>
where
    Transport: AsyncRead + Unpin + Send + Sync + 'static,
{
    type Output = crate::Result<String>;

    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.read_string().await })
    }
}

impl<T> ReceivedBody<'static, T> {
    /// takes the static transport from this received body
    pub fn take_transport(&mut self) -> Option<T> {
        self.transport.take().map(MutCow::unwrap_owned)
    }
}

fn read_raw<Transport>(
    opt_buffer: &mut Option<Vec<u8>>,
    transport: &mut Transport,
    cx: &mut Context<'_>,
    buf: &mut [u8],
) -> Poll<io::Result<usize>>
where
    Transport: AsyncRead + Unpin + Send + Sync + 'static,
{
    match opt_buffer {
        Some(buffer) if !buffer.is_empty() => {
            let len = buffer.len();
            if len > buf.len() {
                trace!(
                    "have {} bytes of pending data but can only use {}",
                    len,
                    buf.len()
                );
                let remaining = buffer.split_off(buf.len());
                buf.copy_from_slice(buffer);
                *buffer = remaining;
                Ready(Ok(buf.len()))
            } else {
                trace!("have {} bytes of pending data, using all of it", len);
                buf[..len].copy_from_slice(buffer);
                *opt_buffer = None;
                match Pin::new(transport).poll_read(cx, &mut buf[len..]) {
                    Ready(Ok(e)) => Ready(Ok(e + len)),
                    Pending => Ready(Ok(len)),
                    other @ Ready(_) => other,
                }
            }
        }

        _ => Pin::new(transport).poll_read(cx, buf),
    }
}

fn chunk_decode(
    remaining: u64,
    mut chunk_total: u64,
    mut total: u64,
    buf: &mut [u8],
    max_len: u64,
) -> io::Result<(ReceivedBodyState, usize, Option<Vec<u8>>)> {
    if buf.is_empty() {
        return Err(io::Error::new(
            ErrorKind::ConnectionAborted,
            "chunked body closed without a last-chunk as per rfc9112 section 7.1",
        ));
    }
    let mut ranges_to_keep = vec![];
    let mut chunk_start = 0u64;
    let mut chunk_end = remaining;
    let (request_body_state, unused) = loop {
        if chunk_end > 2 {
            let keep_start = usize::try_from(chunk_start).unwrap_or(usize::MAX);
            let keep_end = buf
                .len()
                .min(usize::try_from(chunk_end - 2).unwrap_or(usize::MAX));
            ranges_to_keep.push(keep_start..keep_end);
            let new_bytes = (keep_end - keep_start) as u64;
            chunk_total += new_bytes;
            total += new_bytes;
            if total > max_len {
                return Err(io::Error::new(ErrorKind::Unsupported, "content too long"));
            }
        }

        chunk_start = chunk_end;

        let Some(buf_to_read) = slice_from(chunk_start, buf) else {
            break (
                Chunked {
                    remaining: (chunk_start - buf.len() as u64),
                    chunk_total,
                    total,
                },
                None,
            );
        };

        match httparse::parse_chunk_size(buf_to_read) {
            Ok(Status::Complete((framing_bytes, chunk_size))) => {
                chunk_start += framing_bytes as u64;
                chunk_end = 2 + chunk_start + chunk_size;

                if chunk_size == 0 {
                    break (End, slice_from(chunk_end, buf).map(Vec::from));
                }
            }

            Ok(Status::Partial) => {
                break (
                    Chunked {
                        remaining: 0,
                        chunk_total,
                        total,
                    },
                    slice_from(chunk_start, buf).map(Vec::from),
                );
            }

            Err(InvalidChunkSize) => {
                return Err(io::Error::new(ErrorKind::InvalidData, "invalid chunk size"));
            }
        }
    };

    let mut bytes = 0;

    for range_to_keep in ranges_to_keep {
        let new_bytes = bytes + range_to_keep.end - range_to_keep.start;
        buf.copy_within(range_to_keep, bytes);
        bytes = new_bytes;
    }

    Ok((request_body_state, bytes, unused))
}

const STREAM_READ_BUF_LENGTH: usize = 128;
impl<'conn, Transport> Stream for ReceivedBody<'conn, Transport>
where
    Transport: AsyncRead + Unpin + Send + Sync + 'static,
{
    type Item = Vec<u8>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut bytes = 0;
        let mut vec = vec![0; STREAM_READ_BUF_LENGTH];

        loop {
            match Pin::new(&mut *self).poll_read(cx, &mut vec[bytes..]) {
                Pending if bytes == 0 => return Pending,
                Ready(Ok(0)) if bytes == 0 => return Ready(None),
                Pending | Ready(Ok(0)) => {
                    vec.truncate(bytes);
                    return Ready(Some(vec));
                }
                Ready(Ok(new_bytes)) => {
                    bytes += new_bytes;
                    vec.extend(iter::repeat(0).take(bytes + STREAM_READ_BUF_LENGTH - vec.len()));
                }
                Ready(Err(error)) => {
                    log::error!("got {error:?} in ReceivedBody stream");
                    return Ready(None);
                }
            }
        }
    }
}

impl<'conn, Transport> AsyncRead for ReceivedBody<'conn, Transport>
where
    Transport: AsyncRead + Unpin + Send + Sync + 'static,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        trace!("polling received body with state {:?}", &*self.state);
        let (new_body_state, bytes, unused) = match *self.state {
            Start => (
                match self.content_length {
                    Some(0) => End,

                    Some(total_length) if total_length < self.max_len => FixedLength {
                        current_index: 0,
                        total_length,
                    },

                    Some(_) => {
                        return Ready(Err(io::Error::new(
                            ErrorKind::Unsupported,
                            "content too long",
                        )))
                    }

                    None => Chunked {
                        remaining: 0,
                        chunk_total: 0,
                        total: 0,
                    },
                },
                0,
                None,
            ),

            Chunked {
                remaining,
                chunk_total,
                total,
            } => {
                let bytes = ready!(self.read_raw(cx, buf)?);
                let source_buf = &mut buf[..bytes];
                match chunk_decode(remaining, chunk_total, total, source_buf, self.max_len)? {
                    (Chunked { remaining: 0, .. }, 0, Some(unused))
                        if unused.len() == buf.len() =>
                    {
                        // we didn't use any of the bytes, which would result in a pathological loop
                        return Ready(Err(io::Error::new(
                            ErrorKind::Unsupported,
                            "read buffer too short",
                        )));
                    }

                    other => other,
                }
            }

            FixedLength {
                current_index,
                total_length,
            } => {
                let len = buf.len();
                let remaining = usize::try_from(total_length - current_index).unwrap_or(usize::MAX);
                let buf = &mut buf[..len.min(remaining)];
                let bytes = ready!(self.read_raw(cx, buf)?);
                let current_index = current_index + bytes as u64;
                let state = if bytes == 0 || current_index == total_length {
                    End
                } else {
                    FixedLength {
                        current_index,
                        total_length,
                    }
                };

                (state, bytes, None)
            }

            End => (End, 0, None),
        };

        if let Some(unused) = unused {
            if let Some(existing) = &mut *self.buffer {
                existing.extend_from_slice(&unused);
            } else {
                *self.buffer = Some(unused);
            }
        }

        *self.state = new_body_state;

        if *self.state == End {
            if self.on_completion.is_some() && self.owns_transport() {
                let transport = self.transport.take().unwrap().unwrap_owned();
                let on_completion = self.on_completion.take().unwrap();
                on_completion(transport);
            }
            Ready(Ok(bytes))
        } else if bytes == 0 {
            cx.waker().wake_by_ref();
            Pending
        } else {
            Ready(Ok(bytes))
        }
    }
}

impl<'conn, Transport> fmt::Debug for ReceivedBody<'conn, Transport> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestBody")
            .field("state", &*self.state)
            .field("content_length", &self.content_length)
            .field(
                "buffer",
                &self.buffer.as_deref().map(String::from_utf8_lossy),
            )
            .field("on_completion", &self.on_completion.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
#[allow(missing_docs)]
#[doc(hidden)]
pub enum ReceivedBodyState {
    /// initial state
    #[default]
    Start,

    /// read state for a chunked-encoded body. the number of bytes that have been read from the
    /// current chunk is the difference between remaining and total.
    Chunked {
        /// remaining indicates the bytes left _in the current
        /// chunk_. initial state is zero.
        remaining: u64,

        /// chunk_total indicates the size of the current chunk or zero to
        /// indicate that we expect to read a chunk size at the start
        /// of the next bytes. initial state is zero.
        chunk_total: u64,

        /// total indicates the absolute number of bytes read from all chunks
        total: u64,
    },

    /// read state for a fixed-length body.
    FixedLength {
        /// current index represents the bytes that have already been
        /// read. initial state is zero
        current_index: u64,

        /// total length indicates the claimed length, usually
        /// determined by the content-length header
        total_length: u64,
    },

    /// the terminal read state
    End,
}

impl<Transport> From<ReceivedBody<'static, Transport>> for Body
where
    Transport: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    fn from(rb: ReceivedBody<'static, Transport>) -> Self {
        let len = rb.content_length;
        Body::new_streaming(rb, len)
    }
}
