use crate::{http_types::Body, MutCow};
use encoding_rs::Encoding;
use futures_lite::{
    io::{self, BufReader},
    ready, AsyncRead, AsyncReadExt, AsyncWrite, Stream,
};
use httparse::Status;
use std::{
    convert::TryInto,
    fmt::{self, Formatter},
    io::ErrorKind,
    iter,
    pin::Pin,
    task::{Context, Poll},
};

use Poll::{Pending, Ready};
use ReceivedBodyState::{Chunked, End, FixedLength, Start};

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
# use trillium_http::{http_types::Method, Conn};
let mut conn = Conn::new_synthetic(Method::Get, "/", "hello");
let body = conn.request_body().await;
assert_eq!(body.read_string().await?, "hello");
# trillium_http::Result::Ok(()) }).unwrap();
```
*/

pub struct ReceivedBody<'conn, Transport> {
    content_length: Option<u64>,
    buffer: MutCow<'conn, Option<Vec<u8>>>,
    transport: Option<MutCow<'conn, Transport>>,
    state: MutCow<'conn, ReceivedBodyState>,
    on_completion: Option<Box<dyn Fn(Transport) + Send + Sync + 'static>>,
    encoding: &'static Encoding,
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
        Self {
            content_length,
            buffer: buffer.into(),
            transport: Some(transport.into()),
            state: state.into(),
            on_completion,
            encoding,
        }
    }

    /**
    Returns the content-length of this body, if available. This
    usually is derived from the content-length header. If the http
    request or response that this body is attached to uses
    transfer-encoding chunked, this will be None.

    ```rust
    # trillium_testing::block_on(async {
    # use trillium_http::{http_types::Method, Conn};
    let mut conn = Conn::new_synthetic(Method::Get, "/", "hello");
    let body = conn.request_body().await;
    assert_eq!(body.content_length(), Some(5));
    # trillium_http::Result::Ok(()) }).unwrap();
    ```
    */
    pub fn content_length(&self) -> Option<u64> {
        self.content_length
    }

    /**
    Reads the entire body to string, using the encoding determined by
    the content-type (mime) charset. If an encoding problem is
    encountered, the String returned by read_string will contain utf8
    replacement characters.

    Note that this can only be performed once per Conn, as the
    underlying data is not cached anywhere. This is the only copy of
    the body contents.
     */
    pub async fn read_string(self) -> crate::Result<String> {
        let encoding = self.encoding();
        let bytes = self.read_bytes().await?;
        let (s, _, _) = encoding.decode(&bytes);
        Ok(s.to_string())
    }

    fn owns_transport(&self) -> bool {
        self.transport
            .as_ref()
            .map(|transport| transport.is_owned())
            .unwrap_or_default()
    }

    /**
    Similar to [`ReceivedBody::read_string`], but returns the raw bytes. This is
    useful for bodies that are not text.

    You can use this in conjunction with `encoding` if you need
    different handling of malformed character encoding than the lossy
    conversion provided by `read_string`.
    */
    pub async fn read_bytes(mut self) -> crate::Result<Vec<u8>> {
        let mut vec = if let Some(len) = self.content_length {
            Vec::with_capacity(len.try_into().unwrap_or_else(|_| usize::max_value()))
        } else {
            Vec::new()
        };

        self.read_to_end(&mut vec).await?;
        Ok(vec)
    }

    /**
    returns the character encoding of this body, usually
    determined from the content type (mime-type) of the associated
    Conn.
    */
    pub fn encoding(&self) -> &'static Encoding {
        self.encoding
    }

    fn read_raw(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        if let Some(transport) = self.transport.as_mut() {
            read_raw(&mut *self.buffer, &mut **transport, cx, buf)
        } else {
            Ready(Err(ErrorKind::NotConnected.into()))
        }
    }

    /**
    Consumes the remainder of this body from the underlying transport
    by reading it to the end and discarding the contents. This is
    important for http1.1 keepalive, but most of the time you do not
    need to directly call this. It returns the number of bytes
    consumed.
    */
    pub async fn drain(self) -> io::Result<u64> {
        io::copy(self, io::sink()).await
    }
}

impl<T> ReceivedBody<'static, T> {
    /// takes the static transport from this received body
    pub fn take_transport(&mut self) -> Option<T> {
        self.transport.take().map(|t| t.unwrap_owned())
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
                    other => other,
                }
            }
        }

        _ => Pin::new(transport).poll_read(cx, buf),
    }
}

fn chunk_decode(
    remaining: usize,
    mut total: usize,
    buf: &mut [u8],
) -> io::Result<(ReceivedBodyState, usize, Option<Vec<u8>>)> {
    let mut ranges_to_keep = vec![];
    let mut chunk_start = 0;
    let mut chunk_end = remaining;
    let (request_body_state, unused) = loop {
        if chunk_end > 2 {
            let keep_end = buf.len().min(chunk_end - 2);
            ranges_to_keep.push(chunk_start..keep_end);
            total += keep_end - chunk_start;
        }

        chunk_start = chunk_end;

        if chunk_start >= buf.len() {
            break (
                Chunked {
                    remaining: (chunk_start - buf.len()),
                    total,
                },
                None,
            );
        }

        match httparse::parse_chunk_size(&buf[chunk_start..]) {
            Ok(Status::Complete((framing_bytes, chunk_size))) => {
                chunk_start += framing_bytes;
                chunk_end = 2 + chunk_start + chunk_size as usize;

                if chunk_size == 0 {
                    break (
                        End,
                        if chunk_end < buf.len() {
                            Some(buf[chunk_end..].to_vec())
                        } else {
                            None
                        },
                    );
                }
            }

            Ok(Status::Partial) => {
                break (
                    Chunked {
                        remaining: 0,
                        total,
                    },
                    if chunk_start < buf.len() {
                        Some(buf[chunk_start..].to_vec())
                    } else {
                        None
                    },
                );
            }

            Err(httparse::InvalidChunkSize) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid chunk size",
                ));
            }
        }
    };

    let mut bytes = 0;

    for range_to_keep in ranges_to_keep.drain(..) {
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
                _ => panic!(),
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

                    Some(total_length) => FixedLength {
                        current_index: 0,
                        total_length,
                    },

                    None => Chunked {
                        remaining: 0,
                        total: 0,
                    },
                },
                0,
                None,
            ),

            Chunked { remaining, total } => {
                let bytes = ready!(self.read_raw(cx, buf)?);
                chunk_decode(remaining, total, &mut buf[..bytes])?
            }

            FixedLength {
                current_index,
                total_length,
            } => {
                let len = buf.len();
                let remaining = (total_length - current_index) as usize;
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

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
/// the current read state of this body
pub enum ReceivedBodyState {
    /// initial state
    Start,

    /// read state for a chunked-encoded body. the number of bytes that have been read from the
    /// current chunk is the difference between remaining and total.
    Chunked {
        /// remaining indicates the bytes left _in the current
        /// chunk_. initial state is zero.
        remaining: usize,
        /// total indicates the size of the current chunk or zero to
        /// indicate that we expect to read a chunk size at the start
        /// of the next bytes. initial state is zero.
        total: usize,
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

impl Default for ReceivedBodyState {
    fn default() -> Self {
        Start
    }
}

impl<Transport> From<ReceivedBody<'static, Transport>> for Body
where
    Transport: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    fn from(rb: ReceivedBody<'static, Transport>) -> Self {
        let len = rb.content_length.map(|cl| cl as u64);
        Body::from_reader(BufReader::new(rb), len)
    }
}

// This is commented out because I do not have use for it anymore and
// as I was writing out the documentation I realized it was a footgun
// without a clear use case. I'm retaining it in the code in case it's
// useful later, though
//
// impl<'conn, Transport> ReceivedBody<'conn, Transport>
// where
//     Transport: AsyncRead + Unpin + Send + Sync + Clone + 'static,
// {
//     /**
//     When the transport is Clone, this allows the creation of an owned
//     body without taking the original transport away from the Conn.
//
//     Caution: You
//     probably don't want to use this if it can be avoided, as it opens
//     up the potential for two different bodies reading from the same
//     transport, and rust will not protect you from those mistakes.
//      */
//     pub fn into_owned_by_cloning_transport(mut self) -> ReceivedBody<'static, Transport> {
//         ReceivedBody {
//             content_length: self.content_length,
//             buffer: MutCow::Owned(self.buffer.take()),
//             transport: self.transport.map(|transport| MutCow::Owned((*transport).clone())),
//             state: MutCow::Owned(*self.state),
//             on_completion: self.on_completion,
//             encoding: self.encoding,
//         }
//     }
// }

#[cfg(test)]
mod chunk_decode {
    use super::{chunk_decode, ReceivedBody, ReceivedBodyState};
    use encoding_rs::UTF_8;
    use futures_lite::{io::Cursor, AsyncRead, AsyncReadExt};

    fn assert_decoded(input: (usize, &str), expected_output: (Option<usize>, &str, Option<&str>)) {
        let (remaining, input_data) = input;

        let mut buf = input_data.to_string().into_bytes();

        let (output_state, bytes, unused) = chunk_decode(remaining, 0, &mut buf).unwrap();

        assert_eq!(
            (
                match output_state {
                    ReceivedBodyState::Chunked { remaining, .. } => Some(remaining),
                    ReceivedBodyState::End => None,
                    _ => panic!("unexpected output state {:?}", output_state),
                },
                &*String::from_utf8_lossy(&buf[0..bytes]),
                unused.as_deref().map(String::from_utf8_lossy).as_deref()
            ),
            expected_output
        );
    }

    async fn read_with_buffers_of_size<R>(reader: &mut R, size: usize) -> crate::Result<String>
    where
        R: AsyncRead + Unpin,
    {
        let mut return_buffer = vec![];
        loop {
            let mut buf = vec![0; size];
            match reader.read(&mut buf).await? {
                0 => break Ok(String::from_utf8_lossy(&return_buffer).into()),
                bytes_read => return_buffer.extend_from_slice(&buf[..bytes_read]),
            }
        }
    }

    fn full_decode_with_size(
        input: &str,
        poll_size: usize,
    ) -> crate::Result<(String, ReceivedBody<'static, Cursor<&str>>)> {
        let mut rb = ReceivedBody::new(
            None,
            None,
            Cursor::new(input),
            ReceivedBodyState::Chunked {
                remaining: 0,
                total: 0,
            },
            None,
            UTF_8,
        );

        let output = trillium_testing::block_on(read_with_buffers_of_size(&mut rb, poll_size))?;
        Ok((output, rb))
    }

    #[test]
    fn test_full_decode() {
        env_logger::try_init().ok();

        for size in 3..50 {
            let input = "5\r\n12345\r\n1\r\na\r\n2\r\nbc\r\n3\r\ndef\r\n0\r\n";
            let (output, _) = full_decode_with_size(input, size).unwrap();
            assert_eq!(output, "12345abcdef", "size: {}", size);

            let input = "7\r\nMozilla\r\n9\r\nDeveloper\r\n7\r\nNetwork\r\n0\r\n\r\n";
            let (output, _) = full_decode_with_size(input, size).unwrap();
            assert_eq!(output, "MozillaDeveloperNetwork", "size: {}", size);
        }
    }

    #[test]
    fn test_chunk_start() {
        assert_decoded((0, "5\r\n12345\r\n"), (Some(0), "12345", None));
        assert_decoded((0, "F\r\n1"), (Some(14 + 2), "1", None));
        assert_decoded((0, "5\r\n123"), (Some(2 + 2), "123", None));
        assert_decoded((0, "1\r\nX\r\n1\r\nX\r\n"), (Some(0), "XX", None));
        assert_decoded((0, "1\r\nX\r\n1\r\nX\r\n1"), (Some(0), "XX", Some("1")));
        assert_decoded((0, "FFF\r\n"), (Some(0xfff + 2), "", None));
        assert_decoded((10, "hello"), (Some(5), "hello", None));
        assert_decoded(
            (7, "hello\r\nA\r\n world"),
            (Some(4 + 2), "hello world", None),
        );
        assert_decoded(
            (0, "e\r\ntest test test\r\n0\r\n\r\n"),
            (None, "test test test", None),
        );
        assert_decoded(
            (0, "1\r\n_\r\n0\r\n\r\nnext request"),
            (None, "_", Some("next request")),
        );
        assert_decoded((7, "hello\r\n0\r\n\r\n"), (None, "hello", None));
    }
}
