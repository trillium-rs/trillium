use crate::{Body, Buffer, Error, HttpConfig, MutCow, copy, http_config::DEFAULT_CONFIG};
use Poll::{Pending, Ready};
use ReceivedBodyState::{Chunked, End, FixedLength, PartialChunkSize, Start};
use encoding_rs::Encoding;
use futures_lite::{AsyncRead, AsyncReadExt, AsyncWrite, ready};
use std::{
    fmt::{self, Debug, Formatter},
    future::{Future, IntoFuture},
    io::{self, ErrorKind},
    pin::Pin,
    task::{Context, Poll},
};

mod chunked;
mod fixed_length;
mod h3_data;

/// A received http body
///
/// This type represents a body that will be read from the underlying transport, which it may either
/// borrow from a [`Conn`](crate::Conn) or own.
///
/// ```rust
/// # use trillium_testing::HttpTest;
/// let app = HttpTest::new(|mut conn| async move {
///     let body = conn.request_body().await; // send 100-continue if needed
///     let body_string = body.read_string().await.unwrap();
///     conn.with_response_body(format!("received: {body_string}"))
/// });
///
/// app.get("/").block().assert_body("received: ");
/// app.post("/")
///     .with_body("hello")
///     .block()
///     .assert_body("received: hello");
/// ```
///
/// ## Bounds checking
///
/// Every `ReceivedBody` has a maximum length beyond which it will return an error, expressed as a
/// u64. To override this on the specific `ReceivedBody`, use [`ReceivedBody::with_max_len`] or
/// [`ReceivedBody::set_max_len`]
///
/// The default maximum length is currently set to 500mb. In the next semver-minor release, this
/// value will decrease substantially.
///
/// ## Large chunks, small read buffers
///
/// Attempting to read a chunked body with a buffer that is shorter than the chunk size in hex will
/// result in an error. This limitation is temporary.
#[derive(fieldwork::Fieldwork)]
pub struct ReceivedBody<'conn, Transport> {
    /// The content-length of this body, if available. This
    /// usually is derived from the content-length header. If the http
    /// request or response that this body is attached to uses
    /// transfer-encoding chunked, this will be None.
    ///
    /// ```rust
    /// # use trillium_testing::HttpTest;
    /// HttpTest::new(|mut conn| async move {
    ///     let body = conn.request_body().await;
    ///     assert_eq!(body.content_length(), Some(5));
    ///     let body_string = body.read_string().await.unwrap();
    ///     conn.with_status(200)
    ///         .with_response_body(format!("received: {body_string}"))
    /// })
    /// .post("/")
    /// .with_body("hello")
    /// .block()
    /// .assert_ok()
    /// .assert_body("received: hello");
    /// ```
    #[field(get)]
    content_length: Option<u64>,

    buffer: MutCow<'conn, Buffer>,

    transport: Option<MutCow<'conn, Transport>>,

    state: MutCow<'conn, ReceivedBodyState>,

    on_completion: Option<Box<dyn FnOnce(Transport) + Send + Sync + 'static>>,

    /// the character encoding of this body, usually determined from the content type
    /// (mime-type) of the associated Conn.
    #[field(get)]
    encoding: &'static Encoding,

    /// The maximum length that can be read from this body before error
    ///
    /// See also [`HttpConfig::received_body_max_len`]
    #[field(with, get, set)]
    max_len: u64,

    /// The initial buffer capacity allocated when reading the body to bytes or a string
    ///
    /// See [`HttpConfig::received_body_initial_len`]
    #[field(with, get, set)]
    initial_len: usize,

    /// The maximum number of read loops that reading this received body will perform before
    /// yielding back to the runtime
    ///
    /// See [`HttpConfig::copy_loops_per_yield`]
    #[field(with, get, set)]
    copy_loops_per_yield: usize,

    /// Maximum size to pre-allocate based on content-length for buffering this received body
    ///
    /// See [`HttpConfig::received_body_max_preallocate`]
    #[field(with, get, set)]
    max_preallocate: usize,
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
        buffer: impl Into<MutCow<'conn, Buffer>>,
        transport: impl Into<MutCow<'conn, Transport>>,
        state: impl Into<MutCow<'conn, ReceivedBodyState>>,
        on_completion: Option<Box<dyn FnOnce(Transport) + Send + Sync + 'static>>,
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
        buffer: impl Into<MutCow<'conn, Buffer>>,
        transport: impl Into<MutCow<'conn, Transport>>,
        state: impl Into<MutCow<'conn, ReceivedBodyState>>,
        on_completion: Option<Box<dyn FnOnce(Transport) + Send + Sync + 'static>>,
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
            max_preallocate: config.received_body_max_preallocate,
        }
    }

    // pub fn content_length(&self) -> Option<u64> {
    //     self.content_length
    // }

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
        self.transport.as_ref().is_some_and(MutCow::is_owned)
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
                return Err(Error::ReceivedBodyTooLong(self.max_len));
            }

            let len = usize::try_from(len).map_err(|_| Error::ReceivedBodyTooLong(self.max_len))?;

            Vec::with_capacity(len.min(self.max_preallocate))
        } else {
            Vec::with_capacity(self.initial_len)
        };

        self.read_to_end(&mut vec).await?;
        Ok(vec)
    }

    // pub fn encoding(&self) -> &'static Encoding {
    //     self.encoding
    // }

    fn read_raw(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        if let Some(transport) = self.transport.as_deref_mut() {
            read_buffered(&mut self.buffer, transport, cx, buf)
        } else {
            Ready(Err(ErrorKind::NotConnected.into()))
        }
    }

    /// Consumes the remainder of this body from the underlying transport by reading it to the end
    /// and discarding the contents. This is important for http1.1 keepalive, but most of the
    /// time you do not need to directly call this. It returns the number of bytes consumed.
    ///
    /// # Errors
    ///
    /// This will return an [`std::io::Result::Err`] if there is an io error on the underlying
    /// transport, such as a disconnect
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
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;
    type Output = crate::Result<String>;

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

pub(crate) fn read_buffered<Transport>(
    buffer: &mut Buffer,
    transport: &mut Transport,
    cx: &mut Context<'_>,
    buf: &mut [u8],
) -> Poll<io::Result<usize>>
where
    Transport: AsyncRead + Unpin,
{
    if buffer.is_empty() {
        Pin::new(transport).poll_read(cx, buf)
    } else if buffer.len() >= buf.len() {
        let len = buf.len();
        buf.copy_from_slice(&buffer[..len]);
        buffer.ignore_front(len);
        Ready(Ok(len))
    } else {
        let self_buffer_len = buffer.len();
        buf[..self_buffer_len].copy_from_slice(buffer);
        buffer.truncate(0);
        match Pin::new(transport).poll_read(cx, &mut buf[self_buffer_len..]) {
            Ready(Ok(additional)) => Ready(Ok(additional + self_buffer_len)),
            Pending => Ready(Ok(self_buffer_len)),
            other @ Ready(_) => other,
        }
    }
}

type StateOutput = Poll<io::Result<(ReceivedBodyState, usize)>>;

impl<Transport> ReceivedBody<'_, Transport>
where
    Transport: AsyncRead + Unpin + Send + Sync + 'static,
{
    #[inline]
    fn handle_start(&mut self) -> StateOutput {
        Ready(Ok((
            match self.content_length {
                Some(0) => End,

                Some(total_length) if total_length < self.max_len => FixedLength {
                    current_index: 0,
                    total: total_length,
                },

                Some(_) => {
                    return Ready(Err(io::Error::new(
                        ErrorKind::Unsupported,
                        "content too long",
                    )));
                }

                None => Chunked {
                    remaining: 0,
                    total: 0,
                },
            },
            0,
        )))
    }
}

impl<Transport> AsyncRead for ReceivedBody<'_, Transport>
where
    Transport: AsyncRead + Unpin + Send + Sync + 'static,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        for _ in 0..self.copy_loops_per_yield {
            let (new_body_state, bytes) = ready!(match *self.state {
                Start => self.handle_start(),
                Chunked { remaining, total } => self.handle_chunked(cx, buf, remaining, total),
                PartialChunkSize { total } => self.handle_partial(cx, buf, total),
                FixedLength {
                    current_index,
                    total,
                } => self.handle_fixed_length(cx, buf, current_index, total),
                ReceivedBodyState::H3Data {
                    remaining_in_frame,
                    total,
                    frame_type,
                    partial_frame_header,
                } => self.handle_h3_data(
                    cx,
                    buf,
                    remaining_in_frame,
                    total,
                    frame_type,
                    partial_frame_header
                ),
                End => Ready(Ok((End, 0))),
            })?;

            *self.state = new_body_state;

            if *self.state == End {
                if self.on_completion.is_some() && self.owns_transport() {
                    let transport = self.transport.take().unwrap().unwrap_owned();
                    let on_completion = self.on_completion.take().unwrap();
                    on_completion(transport);
                }
                return Ready(Ok(bytes));
            } else if bytes != 0 {
                return Ready(Ok(bytes));
            }
        }

        cx.waker().wake_by_ref();
        Pending
    }
}

impl<Transport> Debug for ReceivedBody<'_, Transport> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestBody")
            .field("state", &*self.state)
            .field("content_length", &self.content_length)
            .field("buffer", &format_args!(".."))
            .field("on_completion", &self.on_completion.is_some())
            .finish()
    }
}

/// The type of H3 frame currently being processed in [`ReceivedBodyState::H3Data`].
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
#[allow(missing_docs)]
#[doc(hidden)]
pub enum H3BodyFrameType {
    /// Initial state — no frame decoded yet.
    #[default]
    Start,
    /// Inside a DATA frame — body bytes to keep.
    Data,
    /// Inside an unknown frame — payload bytes to discard.
    Unknown,
    /// Inside a trailing HEADERS frame — accumulate into buffer for parsing.
    Trailers,
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

        /// total indicates the absolute number of bytes read from all chunks
        total: u64,
    },

    /// read state when we have buffered content between subsequent polls because chunk framing
    /// overlapped a buffer boundary
    PartialChunkSize { total: u64 },

    /// read state for a fixed-length body.
    FixedLength {
        /// current index represents the bytes that have already been
        /// read. initial state is zero
        current_index: u64,

        /// total length indicates the claimed length, usually
        /// determined by the content-length header
        total: u64,
    },

    /// read state for an H3 body framed as DATA frames.
    H3Data {
        /// bytes remaining in the current frame (DATA, Unknown, or Trailers). zero means we need
        /// to read the next frame header.
        remaining_in_frame: u64,

        /// total body bytes read across all DATA frames.
        total: u64,

        /// what kind of frame we're currently inside.
        frame_type: H3BodyFrameType,

        /// when true, a partial frame header is sitting in `self.buffer` and needs more bytes
        /// before we can decode it.
        partial_frame_header: bool,
    },

    /// the terminal read state
    End,
}

impl ReceivedBodyState {
    pub fn new_h3() -> Self {
        Self::H3Data {
            remaining_in_frame: 0,
            total: 0,
            frame_type: H3BodyFrameType::Start,
            partial_frame_header: false,
        }
    }
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
