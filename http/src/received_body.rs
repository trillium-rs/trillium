use crate::{Body, Buffer, Error, Headers, HttpConfig, MutCow, ProtocolSession, copy};
use Poll::{Pending, Ready};
use ReceivedBodyState::{Chunked, End, PartialChunkSize, Raw};
use encoding_rs::Encoding;
use futures_lite::{AsyncRead, AsyncReadExt, AsyncWrite, ready};
use std::{
    fmt::{self, Debug, Formatter},
    io::{self, ErrorKind},
    pin::Pin,
    task::{Context, Poll},
};

mod chunked;
mod h3_data;
mod raw;

pub(crate) use chunked::write_chunk;

/// A received http body
///
/// This type represents a body that will be read from the underlying transport, which it may either
/// borrow from a [`Conn`](crate::Conn) or own.
///
/// ```rust
/// # use trillium_testing::HttpTest;
/// let app = HttpTest::new(|mut conn| async move {
///     let body = conn.request_body();
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
/// The default maximum length is 10mb; see [`HttpConfig::received_body_max_len`] to configure
/// this server-wide.
///
/// ## Large chunks, small read buffers
///
/// Attempting to read a chunked body with a buffer that is shorter than the chunk size in hex will
/// result in an error.
#[derive(fieldwork::Fieldwork)]
pub struct ReceivedBody<'conn, Transport> {
    /// The content-length of this body, derived from the content-length header.
    /// `None` for transfer-encoding chunked bodies.
    ///
    /// ```rust
    /// # use trillium_testing::HttpTest;
    /// HttpTest::new(|mut conn| async move {
    ///     let body = conn.request_body();
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

    max_header_list_size: u64,

    trailers: MutCow<'conn, Option<Headers>>,

    /// Byte offset into `b"HTTP/1.1 100 Continue\r\n\r\n"` that remains to be written before the
    /// first read. `None` means no pending write.
    send_100_continue_offset: Option<usize>,

    /// Protocol session this body belongs to; used by the `End` transition to pull
    /// driver-decoded trailers (h2 synchronously, h3 asynchronously).
    protocol_session: ProtocolSession,

    /// Pending h3 trailer-decode future
    h3_trailer_future: MutCow<'conn, Option<H3TrailerFuture>>,

    /// Accumulator for the QPACK-encoded trailer payload
    h3_trailer_payload_buffer: MutCow<'conn, Vec<u8>>,
}

/// Boxed future returned by the QPACK decoder for trailing HEADERS on an h3 body.
pub(crate) type H3TrailerFuture =
    Pin<Box<dyn Future<Output = io::Result<Headers>> + Send + Sync + 'static>>;

fn slice_from(min: u64, buf: &[u8]) -> Option<&[u8]> {
    buf.get(usize::try_from(min).unwrap_or(usize::MAX)..)
        .filter(|buf| !buf.is_empty())
}

impl<'conn, Transport> ReceivedBody<'conn, Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
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
            &HttpConfig::DEFAULT,
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
            max_header_list_size: config.max_header_list_size,
            trailers: None.into(),
            send_100_continue_offset: None,
            protocol_session: ProtocolSession::Http1,
            h3_trailer_future: None.into(),
            h3_trailer_payload_buffer: Vec::new().into(),
        }
    }

    /// Park the QPACK trailer-decode future in caller-owned storage. Required when this
    /// body is rebuilt per `poll_read` (the future's registered waker would otherwise be
    /// dropped along with the future on `Pending`).
    #[must_use]
    pub(crate) fn with_h3_trailer_future(
        mut self,
        future: impl Into<MutCow<'conn, Option<H3TrailerFuture>>>,
    ) -> Self {
        self.h3_trailer_future = future.into();
        self
    }

    /// Park the QPACK trailer-payload accumulator in caller-owned storage. Required when
    /// this body is rebuilt per `poll_read` so the partial accumulation persists across
    /// polls.
    #[must_use]
    pub(crate) fn with_h3_trailer_payload_buffer(
        mut self,
        buffer: impl Into<MutCow<'conn, Vec<u8>>>,
    ) -> Self {
        self.h3_trailer_payload_buffer = buffer.into();
        self
    }

    /// Sets the destination for trailers decoded from the request body.
    ///
    /// When the body is fully read, any trailers will be written to the provided storage.
    #[doc(hidden)]
    #[must_use]
    pub fn with_trailers(mut self, trailers: impl Into<MutCow<'conn, Option<Headers>>>) -> Self {
        self.trailers = trailers.into();
        self
    }

    /// Associate this body with the [`ProtocolSession`] that produced it. The End
    /// transition of the body state machine consults this to pull driver-decoded
    /// trailers into [`Conn::request_trailers`][crate::Conn] (h2 synchronously,
    /// h3 via a boxed future). For h1 bodies the session is
    /// [`ProtocolSession::Http1`] and no trailer-driver hook fires.
    #[doc(hidden)]
    #[must_use]
    #[cfg(feature = "unstable")]
    pub fn with_protocol_session(mut self, protocol_session: ProtocolSession) -> Self {
        self.protocol_session = protocol_session;
        self
    }

    #[doc(hidden)]
    #[must_use]
    #[cfg(not(feature = "unstable"))]
    pub(crate) fn with_protocol_session(mut self, protocol_session: ProtocolSession) -> Self {
        self.protocol_session = protocol_session;
        self
    }

    /// Arranges for `HTTP/1.1 100 Continue\r\n\r\n` to be written to the transport before the
    /// first body read. Used to implement lazy 100-continue for HTTP/1.1 request bodies.
    #[must_use]
    pub(crate) fn with_send_100_continue(mut self) -> Self {
        self.send_100_continue_offset = Some(0);
        self
    }

    /// # Reads entire body to String.
    ///
    /// This uses the encoding determined by the content-type (mime) charset. If an
    /// encoding problem is encountered, the returned String will contain utf8
    /// replacement characters.
    ///
    /// Can only be performed once per Conn — the body bytes are not cached.
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
    #[allow(
        clippy::missing_errors_doc,
        reason = "errors are documented above; clippy doesn't detect the section"
    )]
    pub async fn drain(self) -> io::Result<u64> {
        let copy_loops_per_yield = self.copy_loops_per_yield;
        copy(self, futures_lite::io::sink(), copy_loops_per_yield).await
    }
}

impl<T> ReceivedBody<'static, T> {
    /// takes the static transport from this received body
    pub fn take_transport(&mut self) -> Option<T> {
        self.transport.take().map(MutCow::unwrap_owned)
    }

    #[doc(hidden)]
    #[cfg(feature = "unstable")]
    pub fn state(&self) -> ReceivedBodyState {
        *self.state
    }
}

impl<T> ReceivedBody<'_, T> {
    /// Borrow the trailers decoded from this body, if any. Unlike [`BodySource::trailers`],
    /// this does not take them. Only `Some` after the body has been read to end-of-stream on
    /// a protocol that carried a trailer section.
    ///
    /// [`BodySource::trailers`]: crate::BodySource::trailers
    #[doc(hidden)]
    #[cfg(feature = "unstable")]
    pub fn trailers_ref(&self) -> Option<&Headers> {
        self.trailers.as_ref()
    }

    /// Retype as `ReceivedBody<'static, T>` if every internal `MutCow` field is `Owned`.
    ///
    /// Returns `None` if any field is `Borrowed`, in which case `self` is dropped — the
    /// borrows can't be extended, and there's no useful way to hand a half-destructured
    /// body back. For callers whose runtime invariants guarantee ownership but whose
    /// type-level `'a` parameter the compiler can't see is `'static`.
    #[doc(hidden)]
    #[cfg(feature = "unstable")]
    pub fn try_into_owned(self) -> Option<ReceivedBody<'static, T>> {
        let Self {
            content_length,
            buffer,
            transport,
            state,
            on_completion,
            encoding,
            max_len,
            initial_len,
            copy_loops_per_yield,
            max_preallocate,
            max_header_list_size,
            trailers,
            send_100_continue_offset,
            protocol_session,
            h3_trailer_future,
            h3_trailer_payload_buffer,
        } = self;

        let transport = match transport {
            None => None,
            Some(t) => Some(t.try_into_owned()?),
        };

        Some(ReceivedBody {
            content_length,
            buffer: buffer.try_into_owned()?,
            transport,
            state: state.try_into_owned()?,
            on_completion,
            encoding,
            max_len,
            initial_len,
            copy_loops_per_yield,
            max_preallocate,
            max_header_list_size,
            trailers: trailers.try_into_owned()?,
            send_100_continue_offset,
            protocol_session,
            h3_trailer_future: h3_trailer_future.try_into_owned()?,
            h3_trailer_payload_buffer: h3_trailer_payload_buffer.try_into_owned()?,
        })
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

impl<Transport> AsyncRead for ReceivedBody<'_, Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        const CONTINUE: &[u8] = b"HTTP/1.1 100 Continue\r\n\r\n";
        while let Some(offset) = self.send_100_continue_offset {
            let n = {
                let Some(transport) = self.transport.as_deref_mut() else {
                    return Ready(Err(ErrorKind::NotConnected.into()));
                };
                if offset == 0 {
                    log::trace!("sending 100-continue");
                }
                ready!(Pin::new(transport).poll_write(cx, &CONTINUE[offset..]))?
            };
            if n == 0 {
                return Ready(Err(ErrorKind::WriteZero.into()));
            }
            let new_offset = offset + n;
            self.send_100_continue_offset = if new_offset >= CONTINUE.len() {
                None
            } else {
                Some(new_offset)
            };
        }

        for _ in 0..self.copy_loops_per_yield {
            let (new_body_state, bytes) = ready!(match *self.state {
                Chunked { remaining, total } => self.handle_chunked(cx, buf, remaining, total),
                PartialChunkSize { total } => self.handle_partial(cx, buf, total),
                Raw { total } => self.handle_raw(cx, buf, total),
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
                    partial_frame_header,
                ),
                ReceivedBodyState::ReadingH1Trailers { total } => {
                    self.handle_reading_h1_trailers(cx, buf, total)
                }
                End => Ready(Ok((End, 0))),
            })?;

            *self.state = new_body_state;

            if *self.state == End {
                if bytes == 0
                    && let Some(h3_trailer_future) = self.h3_trailer_future.as_mut()
                {
                    let trailers = ready!(h3_trailer_future.as_mut().poll(cx))?;
                    *self.trailers = Some(trailers);
                    *self.h3_trailer_future = None;
                }

                // h2 trailers are stashed on the per-stream `StreamState` before EOF, so
                // they're already present at `End` — no boxed future needed. Replacing
                // the session with `Http1` makes subsequent `End` re-entries idempotent.
                if bytes == 0
                    && let Some((h2_connection, stream_id)) =
                        std::mem::replace(&mut self.protocol_session, ProtocolSession::Http1)
                            .as_h2()
                    && let Some(trailers) = h2_connection.take_trailers(stream_id)
                {
                    *self.trailers = Some(trailers);
                }

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

impl<Transport> crate::BodySource for ReceivedBody<'static, Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    fn trailers(self: Pin<&mut Self>) -> Option<Headers> {
        self.get_mut().trailers.take()
    }
}

impl<Transport> Debug for ReceivedBody<'_, Transport> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReceivedBody")
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

    /// Plain payload bytes from the transport, with optional content-length bound. No
    /// framing happens here, just `max_len` / content-length enforcement against a
    /// running total. With a declared length, reads are clamped to the remaining
    /// declared bytes and the state ends at the boundary; without one, ends on EOF.
    ///
    /// Used for HTTP/1.x bodies declared via `Content-Length`, HTTP/2 bodies (the h2
    /// driver demuxes DATA frames into a per-stream receive ring upstream of this),
    /// HTTP/1.0 read-to-close response bodies, and raw upgrade transports (CONNECT,
    /// websockets-over-h1).
    Raw {
        /// total body bytes read.
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

    /// accumulating the HTTP/1.1 chunked trailer-section after the last-chunk (`0\r\n`).
    ///
    /// The trailer bytes (including any partially-received trailer headers) live in
    /// `ReceivedBody::buffer` until a final empty line (`\r\n\r\n` or bare `\r\n`) is found.
    ReadingH1Trailers {
        /// total body bytes read across all chunks (for bounds-checking)
        total: u64,
    },

    /// the terminal read state
    #[default]
    End,
}

impl ReceivedBodyState {
    /// Initial state for an HTTP/1.x body framed via `Content-Length` and/or
    /// `Transfer-Encoding: chunked`. Chunked encoding produces [`Self::Chunked`];
    /// `Some(0)` collapses to [`Self::End`]; everything else — including `None` for
    /// HTTP/1.0 read-to-close — produces [`Self::Raw`], whose handler clamps reads to
    /// the declared length when one is present.
    pub fn new_h1(content_length: Option<u64>, transfer_encoding_chunked: bool) -> Self {
        if transfer_encoding_chunked {
            Self::Chunked {
                remaining: 0,
                total: 0,
            }
        } else if let Some(0) = content_length {
            Self::End
        } else {
            Self::Raw { total: 0 }
        }
    }

    /// Initial state for an HTTP/2 body — [`Self::Raw`] with a zero running total,
    /// since the h2 transport already yields plain payload bytes.
    pub fn new_h2() -> Self {
        Self::Raw { total: 0 }
    }

    /// Initial state for an HTTP/3 body framed as DATA frames.
    pub fn new_h3() -> Self {
        Self::H3Data {
            remaining_in_frame: 0,
            total: 0,
            frame_type: H3BodyFrameType::Start,
            partial_frame_header: false,
        }
    }

    /// Whether the body's read state is one whose first poll has not yet produced any
    /// bytes. False for [`Self::End`] (terminal) and for the intermediate states
    /// [`Self::PartialChunkSize`] / [`Self::ReadingH1Trailers`] that are only reached
    /// after some reading has occurred.
    pub fn is_unread(&self) -> bool {
        matches!(
            self,
            Self::Chunked {
                total: 0,
                remaining: 0
            } | Self::Raw { total: 0 }
                | Self::H3Data { total: 0, .. }
        )
    }
}

impl<Transport> From<ReceivedBody<'static, Transport>> for Body
where
    Transport: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    fn from(rb: ReceivedBody<'static, Transport>) -> Self {
        let len = rb.content_length;
        Body::new_with_trailers(rb, len)
    }
}
