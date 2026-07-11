use crate::{
    Buffer, Conn, Headers, HttpContext, KnownHeaderName, Method, ProtocolSession, ReceivedBody,
    Status, TypeSet, Version,
    h2::H2Connection,
    h3::{Frame, H3Connection},
    headers::qpack::{FieldSection, PseudoHeaders},
    received_body::{H3TrailerFuture, ReceivedBodyState, write_chunk},
    util::encoding,
};
use encoding_rs::Encoding;
use fieldwork::Fieldwork;
use futures_lite::{
    AsyncWriteExt,
    io::{AsyncRead, AsyncWrite},
};
use std::{
    borrow::Cow,
    fmt::{self, Debug, Formatter},
    io::{self, IoSlice, Write},
    net::IpAddr,
    pin::Pin,
    str,
    sync::Arc,
    task::{Context, Poll, ready},
    time::Instant,
};

/// Per-protocol outbound framing state for an [`Upgrade`], chosen at the upgrade
/// transition.
#[derive(Debug)]
pub(crate) enum WriteState {
    /// No framing on the `AsyncWrite` path. HTTP/1.1 without chunked encoding (raw
    /// passthrough) and HTTP/2 (framed at the connection layer).
    Raw,
    /// HTTP/1.1 chunked transfer-encoding.
    H1Chunked(H1ChunkedState),
    /// HTTP/3 DATA-frame encoding.
    H3Framed(H3FramedState),
}

#[derive(Debug, Default)]
pub(crate) struct H1ChunkedState {
    pub(crate) pending: Vec<u8>,
    pub(crate) terminator_written: bool,
}

#[derive(Debug, Default)]
pub(crate) struct H3FramedState {
    pub(crate) pending: Vec<u8>,
    pub(crate) terminator_written: bool,
}

/// Pick outbound framing from http version and the outbound headers' `Transfer-Encoding`.
/// h3 is always DATA-framed; h1 chunks only when the headers request it; h2 is framed by
/// the connection driver, so the `AsyncWrite` path stays raw.
fn compute_write_state(version: Version, outbound_headers: &Headers) -> WriteState {
    match version {
        Version::Http1_0 | Version::Http1_1 if has_chunked_encoding(outbound_headers) => {
            WriteState::H1Chunked(H1ChunkedState::default())
        }
        Version::Http3 => WriteState::H3Framed(H3FramedState::default()),
        _ => WriteState::Raw,
    }
}

/// True if `Transfer-Encoding` includes `chunked`. Tolerant of multi-codings like
/// `gzip, chunked`; no ordering enforcement.
fn has_chunked_encoding(headers: &Headers) -> bool {
    headers
        .token_iter(KnownHeaderName::TransferEncoding)
        .any(|coding| coding.eq_ignore_ascii_case("chunked"))
}

/// Parse the inbound `Content-Length`. `None` for chunked, missing, or malformed.
fn parse_content_length(inbound_headers: &Headers) -> Option<u64> {
    if inbound_headers.has_header(KnownHeaderName::TransferEncoding) {
        return None;
    }
    inbound_headers.content_length()
}

/// Drain `pending` to `transport`, returning `Pending` if the transport blocks.
fn poll_drain_pending<T: AsyncWrite + Unpin>(
    pending: &mut Vec<u8>,
    cx: &mut Context<'_>,
    transport: &mut T,
) -> Poll<io::Result<()>> {
    while !pending.is_empty() {
        match Pin::new(&mut *transport).poll_write(cx, pending) {
            Poll::Ready(Ok(0)) => return Poll::Ready(Err(io::ErrorKind::WriteZero.into())),
            Poll::Ready(Ok(n)) => {
                pending.drain(..n);
            }
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => return Poll::Pending,
        }
    }
    Poll::Ready(Ok(()))
}

/// Drain `pending` until the transport blocks or `pending` is empty, without yielding
/// `Pending`. The next call resumes the drain.
fn best_effort_drain<T: AsyncWrite + Unpin>(
    pending: &mut Vec<u8>,
    cx: &mut Context<'_>,
    transport: &mut T,
) -> io::Result<()> {
    while !pending.is_empty() {
        match Pin::new(&mut *transport).poll_write(cx, pending) {
            Poll::Ready(Ok(0)) => return Err(io::ErrorKind::WriteZero.into()),
            Poll::Ready(Ok(n)) => {
                pending.drain(..n);
            }
            Poll::Ready(Err(e)) => return Err(e),
            Poll::Pending => break,
        }
    }
    Ok(())
}

/// Append an HTTP/3 DATA frame header for `payload_len` bytes to `out`. Caller appends
/// the payload immediately after.
fn encode_h3_data_header(out: &mut Vec<u8>, payload_len: u64) {
    let frame = Frame::Data(payload_len);
    let header_len = frame.encoded_len();
    let start = out.len();
    out.resize(start + header_len, 0);
    frame.encode(&mut out[start..]);
}

/// An HTTP upgrade — owns the underlying transport along with all the data from the
/// originating [`Conn`].
///
/// **Reading the transport directly**: drain `buffer` first if it has bytes in it. Reading
/// via the [`AsyncRead`] impl on `Upgrade` handles this automatically.
#[derive(Fieldwork)]
#[fieldwork(get, get_mut, set, with, take, into_field, rename_predicates)]
pub struct Upgrade<Transport> {
    /// The http headers the peer sent to us
    #[field(deprecate(was = "request_headers", since = "1.3.0"))]
    pub(crate) received_headers: Headers,

    /// The http headers as set before the upgrade was negotiated and sent
    /// to the peer.
    #[field(deprecate(was = "response_headers", since = "1.3.0"))]
    pub(crate) sent_headers: Headers,

    /// The request path
    #[field(get = false)]
    pub(crate) path: Cow<'static, str>,

    /// The http request method
    #[field(copy)]
    pub(crate) method: Method,

    /// Any state that has been accumulated on the Conn before negotiating the upgrade
    pub(crate) state: TypeSet,

    /// The underlying io (often a `TcpStream` or similar)
    pub(crate) transport: Transport,

    /// Any bytes that have been read from the underlying transport already.
    ///
    /// It is your responsibility to process these bytes before reading directly from the
    /// transport.
    #[field(deref = "[u8]", into_field = false, set = false, with = false)]
    pub(crate) buffer: Buffer,

    /// The [`HttpContext`] shared for this server
    #[field(deref = false)]
    pub(crate) context: Arc<HttpContext>,

    /// the ip address of the connection, if available
    #[field(copy)]
    pub(crate) peer_ip: Option<IpAddr>,

    /// the wall-clock time at which the underlying [`Conn`] was constructed
    #[field(copy)]
    pub(crate) start_time: Instant,

    /// the :authority http/3 pseudo-header
    pub(crate) authority: Option<Cow<'static, str>>,

    /// the :scheme http/3 pseudo-header
    pub(crate) scheme: Option<Cow<'static, str>>,

    /// the [`ProtocolSession`] for this upgrade — h2/h3 connection driver + stream id
    /// where applicable; `Http1` for upgrades from h1 or synthetic conns.
    #[field = false]
    pub(crate) protocol_session: ProtocolSession,

    /// the :protocol http/3 pseudo-header
    pub(crate) protocol: Option<Cow<'static, str>>,

    /// the http version
    #[field = "http_version"]
    pub(crate) version: Version,

    /// the http response status set on the underlying [`Conn`] before the upgrade
    /// (typically `101 Switching Protocols`, or `200 OK` for CONNECT). `None` if unset.
    #[field(copy)]
    pub(crate) status: Option<Status>,

    /// whether this connection was deemed secure by the handler stack
    pub(crate) secure: bool,

    /// Inbound framing state carried across the upgrade so the inbound state machine
    /// resumes where the pre-upgrade handler left off. Request-body state on server
    /// upgrades; response-body state on client upgrades.
    #[field = false]
    pub(crate) received_body_state: ReceivedBodyState,

    /// Inbound trailers, populated either by a fully-consumed pre-upgrade body or by
    /// the post-upgrade inbound state machine. `Some` only when non-empty.
    #[field(get, get_mut, take, set = false, with = false, into_field = false)]
    pub(crate) received_trailers: Option<Headers>,

    /// Pre-parsed inbound `Content-Length`. `None` for chunked, missing, or malformed.
    #[field = false]
    pub(crate) content_length_in: Option<u64>,

    /// Per-protocol outbound framing state. Decided at the upgrade transition.
    #[field = false]
    pub(crate) write_state: WriteState,

    /// Charset of the inbound body, parsed from the inbound `Content-Type`'s `charset`
    /// parameter at the upgrade transition.
    #[field = false]
    pub(crate) inbound_encoding: &'static Encoding,

    /// In-flight QPACK trailer-decode future for inbound h3 trailing HEADERS. Held here
    /// so its registered waker survives across `poll_read` calls — dropping the future
    /// would drop the waker the QPACK decoder is parked on, hanging the reader.
    #[field = false]
    pub(crate) h3_trailer_decode_in: Option<H3TrailerFuture>,

    /// Accumulator for inbound h3 trailing-HEADERS payload bytes pre-QPACK-decode.
    /// Separate from [`Self::buffer`] so the inbound state machine doesn't recycle
    /// accumulated trailer bytes back through the frame decoder and double-count them.
    #[field = false]
    pub(crate) h3_trailer_payload_in: Vec<u8>,
}

impl<Transport> Upgrade<Transport> {
    #[doc(hidden)]
    pub fn new(
        received_headers: Headers,
        path: impl Into<Cow<'static, str>>,
        method: Method,
        transport: Transport,
        buffer: Buffer,
        version: Version,
    ) -> Self {
        Self {
            received_headers,
            sent_headers: Headers::new(),
            path: path.into(),
            method,
            transport,
            buffer,
            state: TypeSet::new(),
            context: Arc::default(),
            peer_ip: None,
            start_time: Instant::now(),
            authority: None,
            scheme: None,
            protocol_session: ProtocolSession::Http1,
            protocol: None,
            secure: false,
            version,
            status: None,
            received_body_state: ReceivedBodyState::Raw { total: 0 },
            received_trailers: None,
            content_length_in: None,
            write_state: WriteState::Raw,
            inbound_encoding: encoding_rs::WINDOWS_1252,
            h3_trailer_decode_in: None,
            h3_trailer_payload_in: Vec::new(),
        }
    }

    #[cfg(feature = "unstable")]
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        received_headers: Headers,
        sent_headers: Headers,
        path: Cow<'static, str>,
        method: Method,
        transport: Transport,
        buffer: Buffer,
        state: TypeSet,
        context: Arc<HttpContext>,
        peer_ip: Option<IpAddr>,
        authority: Option<Cow<'static, str>>,
        scheme: Option<Cow<'static, str>>,
        protocol_session: ProtocolSession,
        protocol: Option<Cow<'static, str>>,
        version: Version,
        status: Option<Status>,
        secure: bool,
        received_body_state: ReceivedBodyState,
        received_trailers: Option<Headers>,
    ) -> Self {
        let write_state = compute_write_state(version, &sent_headers);
        let content_length_in = parse_content_length(&received_headers);
        let inbound_encoding = encoding(&received_headers);

        Self {
            received_headers,
            sent_headers,
            path,
            method,
            state,
            transport,
            buffer,
            context,
            peer_ip,
            start_time: Instant::now(),
            authority,
            scheme,
            protocol_session,
            protocol,
            version,
            status,
            secure,
            received_body_state,
            received_trailers,
            content_length_in,
            write_state,
            inbound_encoding,
            h3_trailer_decode_in: None,
            h3_trailer_payload_in: Vec::new(),
        }
    }

    /// the [`H2Connection`] driver for this upgrade, if it originated from an HTTP/2 stream
    pub fn h2_connection(&self) -> Option<&Arc<H2Connection>> {
        self.protocol_session.h2_connection()
    }

    /// the h2 stream id for this upgrade, if it originated from an HTTP/2 stream
    pub fn h2_stream_id(&self) -> Option<u32> {
        self.protocol_session.h2_stream_id()
    }

    /// the [`H3Connection`] driver for this upgrade, if it originated from an HTTP/3 stream
    pub fn h3_connection(&self) -> Option<&Arc<H3Connection>> {
        self.protocol_session.h3_connection()
    }

    /// the h3 stream id for this upgrade, if it originated from an HTTP/3 stream
    pub fn h3_stream_id(&self) -> Option<u64> {
        self.protocol_session.h3_stream_id()
    }

    /// Take any buffered bytes
    pub fn take_buffer(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buffer).into()
    }

    #[doc(hidden)]
    pub fn buffer_and_transport_mut(&mut self) -> (&mut Buffer, &mut Transport) {
        (&mut self.buffer, &mut self.transport)
    }

    /// borrow the shared state [`TypeSet`] for this application
    pub fn shared_state(&self) -> &TypeSet {
        self.context.shared_state()
    }

    /// the http request path up to but excluding any query component
    pub fn path(&self) -> &str {
        match self.path.split_once('?') {
            Some((path, _)) => path,
            None => &self.path,
        }
    }

    /// retrieves the query component of the path
    pub fn querystring(&self) -> &str {
        self.path
            .split_once('?')
            .map(|(_, query)| query)
            .unwrap_or_default()
    }

    /// Modify the transport type of this upgrade.
    ///
    /// This is useful for boxing the transport in order to erase the type argument.
    pub fn map_transport<T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static>(
        self,
        f: impl Fn(Transport) -> T,
    ) -> Upgrade<T> {
        // Manual respread: rustc rejects `..self` across a type parameter change without
        // the unstable `type_changing_struct_update` feature. New fields on `Upgrade`
        // need to be added here, in `Conn::map_transport`, and in `From<Conn> for Upgrade`.
        Upgrade {
            transport: f(self.transport),
            path: self.path,
            method: self.method,
            state: self.state,
            buffer: self.buffer,
            received_headers: self.received_headers,
            sent_headers: self.sent_headers,
            context: self.context,
            peer_ip: self.peer_ip,
            start_time: self.start_time,
            authority: self.authority,
            scheme: self.scheme,
            protocol_session: self.protocol_session,
            protocol: self.protocol,
            version: self.version,
            status: self.status,
            secure: self.secure,
            received_body_state: self.received_body_state,
            received_trailers: self.received_trailers,
            content_length_in: self.content_length_in,
            write_state: self.write_state,
            inbound_encoding: self.inbound_encoding,
            h3_trailer_decode_in: self.h3_trailer_decode_in,
            h3_trailer_payload_in: self.h3_trailer_payload_in,
        }
    }
}

impl<Transport: AsyncWrite + Unpin> Upgrade<Transport> {
    /// Emit trailing headers and finish the outbound stream. Consumes `self`; further
    /// writes are statically prevented.
    ///
    /// Per-protocol behavior:
    /// - HTTP/1.1 with `Transfer-Encoding: chunked`: writes the last-chunk marker (`0\r\n`), the
    ///   trailer section, and a final CRLF, then closes the transport.
    /// - HTTP/2: enqueues a trailing `HEADERS` frame with `END_STREAM` via the connection driver
    ///   and returns. The driver finishes the stream after draining any pending DATA frames.
    /// - HTTP/3: encodes a trailing `HEADERS` frame via QPACK, writes it to the stream, then closes
    ///   the stream (QUIC `FIN`).
    /// - HTTP/1.1 without chunked encoding (raw upgrade, CONNECT tunnel, websocket-over-h1):
    ///   trailers can't be expressed on the wire; dropped with a `log::warn!` and `Ok(())`
    ///   returned.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`io::Error`] when the wire write fails, `BrokenPipe` if
    /// the stream has already been closed, and `NotConnected` if the carried
    /// `ProtocolSession` is missing the expected driver for h2/h3.
    pub async fn send_trailers(self, trailers: Headers) -> io::Result<()> {
        let Self {
            mut transport,
            mut write_state,
            context,
            protocol_session,
            ..
        } = self;

        match &mut write_state {
            WriteState::H1Chunked(state) => {
                if state.terminator_written {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
                state.pending.extend_from_slice(b"0\r\n");
                crate::conn::write_headers_or_trailers(&mut state.pending, &trailers, &context)
                    .map_err(io::Error::other)?;
                state.pending.extend_from_slice(b"\r\n");
                state.terminator_written = true;

                transport.write_all(&state.pending).await?;
                state.pending.clear();
                transport.close().await
            }
            WriteState::H3Framed(state) => {
                if state.terminator_written {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
                let Some((h3, stream_id)) = protocol_session.as_h3() else {
                    return Err(io::ErrorKind::NotConnected.into());
                };
                let field_section = FieldSection::new(PseudoHeaders::default(), &trailers);
                h3.encode_field_section_framed(&field_section, &mut state.pending, stream_id)?;
                state.terminator_written = true;

                transport.write_all(&state.pending).await?;
                state.pending.clear();
                transport.close().await
            }
            WriteState::Raw => {
                if let Some((h2, stream_id)) = protocol_session.as_h2() {
                    h2.submit_trailers(stream_id, trailers)
                } else {
                    log::warn!(
                        "Upgrade::send_trailers called on a raw upgrade with no per-stream \
                         framing; trailers dropped. Set `Transfer-Encoding: chunked` on the \
                         outbound headers if you intend to emit trailers over HTTP/1.1."
                    );
                    Ok(())
                }
            }
        }
    }
}

impl<Transport> Debug for Upgrade<Transport> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(&format!("Upgrade<{}>", std::any::type_name::<Transport>()))
            .field("received_headers", &self.received_headers)
            .field("sent_headers", &self.sent_headers)
            .field("path", &self.path)
            .field("method", &self.method)
            .field("buffer", &self.buffer)
            .field("context", &self.context)
            .field("state", &self.state)
            .field("transport", &format_args!(".."))
            .field("peer_ip", &self.peer_ip)
            .field("start_time", &self.start_time)
            .field("authority", &self.authority)
            .field("scheme", &self.scheme)
            .field("protocol_session", &self.protocol_session)
            .field("protocol", &self.protocol)
            .field("version", &self.version)
            .field("status", &self.status)
            .field("secure", &self.secure)
            .field("received_body_state", &self.received_body_state)
            .field("received_trailers", &self.received_trailers)
            .field("content_length_in", &self.content_length_in)
            .field("write_state", &self.write_state)
            .field("inbound_encoding", &self.inbound_encoding.name())
            .field(
                "h3_trailer_decode_in",
                &self
                    .h3_trailer_decode_in
                    .as_ref()
                    .map(|_| format_args!("..")),
            )
            .field(
                "h3_trailer_payload_in_len",
                &self.h3_trailer_payload_in.len(),
            )
            .finish()
    }
}

impl<Transport> From<Conn<Transport>> for Upgrade<Transport> {
    fn from(conn: Conn<Transport>) -> Self {
        // Exhaustive destructure so new fields on `Conn` force a deliberate carry-vs-drop
        // decision. Shared drift hazard with `Conn::map_transport` and `Upgrade::map_transport`.
        let Conn {
            request_headers,
            response_headers,
            path,
            method,
            state,
            transport,
            buffer,
            context,
            peer_ip,
            start_time,
            authority,
            scheme,
            protocol_session,
            protocol,
            version,
            status,
            secure,
            request_body_state,
            request_trailers,
            response_body,
            // post-send hooks no longer apply; `upgrade` is the marker that brought us here
            after_send: _,
            upgrade: _,
        } = conn;

        if let Some(body) = &response_body
            && !body.is_empty()
        {
            log::warn!(
                "Conn::upgrade() and a non-empty response body are both set; body is being \
                 discarded. The upgrade path is mutually exclusive with serving a response body."
            );
        }

        // Server-side roles: outbound = response_headers, inbound = request_headers.
        let write_state = compute_write_state(version, &response_headers);
        let content_length_in = parse_content_length(&request_headers);
        let inbound_encoding = encoding(&request_headers);
        // An h1 request with no framing headers parses to `End` — correct for the request
        // body, but inherited across the upgrade it would EOF the first read of a live raw
        // stream (a browser websocket handshake is exactly this shape). Declared framing
        // does carry over: a chunked request keeps chunked inbound framing.
        let received_body_state = if matches!(version, Version::Http1_0 | Version::Http1_1)
            && !request_headers.has_header(KnownHeaderName::TransferEncoding)
            && !request_headers.has_header(KnownHeaderName::ContentLength)
        {
            ReceivedBodyState::Raw { total: 0 }
        } else {
            request_body_state
        };
        let received_trailers = request_trailers.filter(|t| !t.is_empty());

        Self {
            received_headers: request_headers,
            sent_headers: response_headers,
            path,
            method,
            state,
            transport,
            buffer,
            context,
            peer_ip,
            start_time,
            authority,
            scheme,
            protocol_session,
            protocol,
            version,
            status,
            secure,
            received_body_state,
            received_trailers,
            content_length_in,
            write_state,
            inbound_encoding,
            h3_trailer_decode_in: None,
            h3_trailer_payload_in: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests;

impl<Transport> AsyncRead for Upgrade<Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let Self {
            transport,
            buffer,
            received_body_state,
            content_length_in,
            context,
            protocol_session,
            received_trailers,
            h3_trailer_decode_in,
            h3_trailer_payload_in,
            inbound_encoding,
            ..
        } = &mut *self;

        let protocol_session = protocol_session.clone();
        let mut body: ReceivedBody<'_, Transport> = ReceivedBody::new_with_config(
            *content_length_in,
            buffer,
            transport,
            received_body_state,
            None,
            inbound_encoding,
            &context.config,
        )
        .with_trailers(received_trailers)
        .with_protocol_session(protocol_session)
        .with_h3_trailer_future(h3_trailer_decode_in)
        .with_h3_trailer_payload_buffer(h3_trailer_payload_in);

        Pin::new(&mut body).poll_read(cx, buf)
    }
}

impl<Transport: AsyncWrite + Unpin> AsyncWrite for Upgrade<Transport> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let Self {
            transport,
            write_state,
            ..
        } = &mut *self;
        match write_state {
            WriteState::Raw => Pin::new(transport).poll_write(cx, buf),
            WriteState::H1Chunked(state) => {
                ready!(poll_drain_pending(&mut state.pending, cx, transport))?;

                // Empty buf must not become a chunk: `0\r\n` IS the last-chunk marker.
                if buf.is_empty() {
                    return Poll::Ready(Ok(0));
                }

                if state.terminator_written {
                    return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
                }

                write_chunk(&mut state.pending, buf);
                best_effort_drain(&mut state.pending, cx, transport)?;
                Poll::Ready(Ok(buf.len()))
            }
            WriteState::H3Framed(state) => {
                ready!(poll_drain_pending(&mut state.pending, cx, transport))?;

                if buf.is_empty() {
                    return Poll::Ready(Ok(0));
                }

                if state.terminator_written {
                    return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
                }

                encode_h3_data_header(&mut state.pending, buf.len() as u64);
                state.pending.extend_from_slice(buf);
                best_effort_drain(&mut state.pending, cx, transport)?;
                Poll::Ready(Ok(buf.len()))
            }
        }
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let Self {
            transport,
            write_state,
            ..
        } = &mut *self;
        match write_state {
            WriteState::Raw => Pin::new(transport).poll_write_vectored(cx, bufs),
            WriteState::H1Chunked(state) => {
                ready!(poll_drain_pending(&mut state.pending, cx, transport))?;
                let total: usize = bufs.iter().map(|b| b.len()).sum();
                if total == 0 {
                    return Poll::Ready(Ok(0));
                }
                if state.terminator_written {
                    return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
                }
                // One chunk per vectored batch — the default impl would emit one chunk per
                // iobuf, which is wasteful when the caller meant them as one logical write.
                let _ = write!(state.pending, "{total:X}\r\n");
                for b in bufs {
                    state.pending.extend_from_slice(b);
                }
                state.pending.extend_from_slice(b"\r\n");
                best_effort_drain(&mut state.pending, cx, transport)?;
                Poll::Ready(Ok(total))
            }
            WriteState::H3Framed(state) => {
                ready!(poll_drain_pending(&mut state.pending, cx, transport))?;
                let total: usize = bufs.iter().map(|b| b.len()).sum();
                if total == 0 {
                    return Poll::Ready(Ok(0));
                }
                if state.terminator_written {
                    return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
                }
                // One DATA frame per vectored batch — collapses `[length_prefix, payload]`
                // pairs into a single frame.
                encode_h3_data_header(&mut state.pending, total as u64);
                for b in bufs {
                    state.pending.extend_from_slice(b);
                }
                best_effort_drain(&mut state.pending, cx, transport)?;
                Poll::Ready(Ok(total))
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let Self {
            transport,
            write_state,
            ..
        } = &mut *self;
        match write_state {
            WriteState::Raw => Pin::new(transport).poll_flush(cx),
            WriteState::H1Chunked(state) => {
                ready!(poll_drain_pending(&mut state.pending, cx, transport))?;
                Pin::new(transport).poll_flush(cx)
            }
            WriteState::H3Framed(state) => {
                ready!(poll_drain_pending(&mut state.pending, cx, transport))?;
                Pin::new(transport).poll_flush(cx)
            }
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let Self {
            transport,
            write_state,
            ..
        } = &mut *self;
        match write_state {
            WriteState::Raw => Pin::new(transport).poll_close(cx),
            WriteState::H1Chunked(state) => {
                ready!(poll_drain_pending(&mut state.pending, cx, transport))?;
                if !state.terminator_written {
                    state.pending.extend_from_slice(b"0\r\n\r\n");
                    // Flag set before the drain so a re-poll after Pending doesn't re-append.
                    state.terminator_written = true;
                }
                ready!(poll_drain_pending(&mut state.pending, cx, transport))?;
                Pin::new(transport).poll_close(cx)
            }
            WriteState::H3Framed(state) => {
                // h3 stream-end is the QUIC FIN — no separate terminator frame.
                ready!(poll_drain_pending(&mut state.pending, cx, transport))?;
                state.terminator_written = true;
                Pin::new(transport).poll_close(cx)
            }
        }
    }
}
