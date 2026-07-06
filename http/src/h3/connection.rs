mod peer_settings_wait;

use super::{
    H3Error,
    frame::{Frame, FrameDecodeError, UniStreamType},
    quic_varint::{self, QuicVarIntError},
    settings::H3Settings,
};
use crate::{
    Buffer, Conn, HttpContext, KnownHeaderName, Priority,
    conn::H3FirstFrame,
    h3::{H3ErrorCode, MAX_BUFFER_SIZE},
    headers::qpack::{DecoderDynamicTable, EncoderDynamicTable, FieldSection},
};
use event_listener::Event;
use futures_lite::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use std::{
    future::{Future, IntoFuture},
    io::{self, ErrorKind},
    pin::Pin,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll},
};
use swansong::{ShutdownCompletion, Swansong};

/// The result of processing an HTTP/3 bidirectional stream.
#[derive(Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "Request is the hot path; boxing it would add an allocation per request"
)]
pub enum H3StreamResult<Transport> {
    /// The stream carried a normal HTTP/3 request.
    Request(Conn<Transport>),

    /// The stream carries a WebTransport bidirectional data stream. The `session_id` identifies
    /// the associated WebTransport session.
    WebTransport {
        /// The WebTransport session ID (stream ID of the CONNECT request).
        session_id: u64,
        /// The underlying transport, ready for application data.
        transport: Transport,
        /// Any bytes buffered after the session ID during stream negotiation.
        buffer: Buffer,
    },
}

/// Inner-loop result of [`H3Connection::process_inbound_uni_with_close`] before the recv
/// stream is reattached. Decouples the inner async block (which only borrows the stream)
/// from the caller-visible [`UniStreamResult`] (which returns the stream by value on
/// non-`Handled` variants), so the function can keep ownership of `stream` long enough to
/// fire its close callback before `stream` drops.
enum UniInnerResult {
    Handled,
    WebTransport { session_id: u64, buffer: Buffer },
    Unknown { stream_type: u64 },
}

/// The result of processing an HTTP/3 unidirectional stream.
#[derive(Debug)]
pub enum UniStreamResult<T> {
    /// The stream was a known internal type (control, QPACK encoder/decoder) and was handled
    /// automatically.
    Handled,

    /// A WebTransport unidirectional data stream. The `session_id` identifies the associated
    /// WebTransport session.
    WebTransport {
        /// The WebTransport session ID.
        session_id: u64,
        /// The receive stream, ready for application data.
        stream: T,
        /// Any bytes buffered after the session ID during stream negotiation.
        buffer: Buffer,
    },

    /// A stream whose type is recognized but unsupported (e.g. `Push`) or not recognized
    /// at all by this crate.
    ///
    /// The caller is responsible for disposing of the stream — the in-tree consumers RST
    /// it with `H3_STREAM_CREATION_ERROR`. `process_inbound_uni` deliberately does *not*
    /// close the stream itself: handing it back gives a downstream extension the option to
    /// implement a stream type trillium-http doesn't know about (a future RFC, an
    /// experiment, etc.) without forking the codec.
    Unknown {
        /// The raw stream type value.
        stream_type: u64,
        /// The stream.
        stream: T,
    },
}

/// Shared state for a single HTTP/3 QUIC connection.
///
/// Call the appropriate methods on this type for each stream accepted from the QUIC connection.
///
/// # Driver shape (vs h2)
///
/// h2 multiplexes everything onto a single TCP byte stream, so a single
/// [`H2Driver`][crate::h2::H2Driver] task suffices. h3 instead has the QUIC layer hand us multiple
/// independent streams: an inbound and outbound control stream, an inbound and outbound QPACK
/// encoder stream, an inbound and outbound QPACK decoder stream, and one bidi stream per
/// request. There is no single "h3 driver" — each stream is driven by its own future returned from
/// `H3Connection`'s `run_*` / `process_*` methods, and the caller decides how those futures are
/// scheduled.
///
/// The trillium-http boundary is **runtime-free by design**: this crate hands out anonymous futures
/// and lets the caller pick the executor. The in-tree consumers (`trillium-server-common`,
/// `trillium-client`) follow a task-per-stream pattern — spawn each long-lived control / encoder /
/// decoder future on its own task at connection setup, then spawn one task per accepted request
/// stream. Nothing in this crate requires that pattern; a caller could in principle race all the
/// futures on one task instead, with different perf characteristics.
#[derive(Debug)]
pub struct H3Connection {
    /// Shared configuration across all protocols.
    context: Arc<HttpContext>,

    /// Connection-scoped shutdown signal. Shut down when we receive GOAWAY from the peer or when
    /// the server-level Swansong shuts down.  Request stream tasks use this to interrupt
    /// in-progress work.
    swansong: Swansong,

    /// The peer's H3 settings, received on their control stream.  Request streams may need to
    /// consult these (e.g. max field section size).
    pub(super) peer_settings: OnceLock<H3Settings>,

    /// Multi-listener wake source for
    /// [`PeerSettingsReady`][peer_settings_wait::PeerSettingsReady]. Notified by
    /// `run_inbound_control` after applying peer SETTINGS, and again on connection
    /// close, so any number of concurrently-parked futures all unblock together.
    pub(super) peer_settings_event: Event,

    /// The highest bidirectional stream ID we have accepted.  Used to compute the GOAWAY value
    /// (this + 4) to tell the peer which requests we saw. None until the first stream is accepted.
    /// Updated by the runtime adapter's accept loop via [`record_accepted_stream`].
    max_accepted_stream_id: AtomicU64,

    /// Whether we have accepted any streams yet.
    has_accepted_stream: AtomicBool,

    /// The decoder-side QPACK dynamic table for this connection.
    decoder_dynamic_table: DecoderDynamicTable,

    /// The encoder-side QPACK dynamic table for this connection.
    encoder_dynamic_table: EncoderDynamicTable,

    /// Sink for RFC 9218 priority signals, set via
    /// [`register_priority_callback`][Self::register_priority_callback]. Unset until the runtime
    /// adapter that owns the QUIC streams registers it.
    priority_callback: PriorityCallback,
}

/// Boxed sink for `(stream_id, priority, is_update)` signals.
type PriorityCallbackFn = Box<dyn Fn(u64, Priority, bool) + Send + Sync>;

/// A registered sink for `(stream_id, priority, is_update)` signals. Newtype so [`H3Connection`]
/// can keep deriving `Debug` despite holding a boxed closure.
#[derive(Default)]
struct PriorityCallback(OnceLock<PriorityCallbackFn>);

impl std::fmt::Debug for PriorityCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("PriorityCallback")
            .field(&self.0.get().map(|_| format_args!("..")))
            .finish()
    }
}

impl H3Connection {
    /// Construct a new `H3Connection` to manage HTTP/3 for a given peer.
    pub fn new(context: Arc<HttpContext>) -> Arc<Self> {
        let swansong = context.swansong.child();
        let max_table_capacity = context.config.dynamic_table_capacity;
        let blocked_streams = context.config.h3_blocked_streams;
        let encoder_dynamic_table = EncoderDynamicTable::new(&context);
        Arc::new(Self {
            context,
            swansong,
            peer_settings: OnceLock::new(),
            peer_settings_event: Event::new(),
            max_accepted_stream_id: AtomicU64::new(0),
            has_accepted_stream: AtomicBool::new(false),
            decoder_dynamic_table: DecoderDynamicTable::new(max_table_capacity, blocked_streams),
            encoder_dynamic_table,
            priority_callback: PriorityCallback::default(),
        })
    }

    /// Register the sink for RFC 9218 priority signals on this connection.
    ///
    /// The callback is invoked with `(stream_id, priority, is_update)` once per request when its
    /// initial `priority` header is parsed (`is_update = false`), and again for every
    /// `PRIORITY_UPDATE` received afterward (`is_update = true`). `is_update` lets the receiver
    /// honor RFC 9218 precedence: a `PRIORITY_UPDATE` outranks the request's initial header
    /// priority regardless of arrival order, including when it arrives before the stream is
    /// accepted.
    ///
    /// This crate keeps no priority state of its own and does no scheduling: it parses each
    /// signal and hands the [`Priority`] off through this callback, leaving the receiver to apply
    /// it to whatever owns send scheduling. Without a registered callback, priority is parsed but
    /// never applied.
    ///
    /// Has no effect if a callback is already registered.
    pub fn register_priority_callback(
        &self,
        callback: impl Fn(u64, Priority, bool) + Send + Sync + 'static,
    ) {
        let _ = self.priority_callback.0.set(Box::new(callback));
    }

    /// Emit a priority signal for a request stream to the registered callback, if any.
    /// `is_update` distinguishes a received `PRIORITY_UPDATE` from the request's initial header
    /// priority so the receiver can honor the precedence rule.
    fn emit_priority(&self, stream_id: u64, priority: Priority, is_update: bool) {
        let kind = if is_update {
            "PRIORITY_UPDATE"
        } else {
            "initial"
        };
        match self.priority_callback.0.get() {
            Some(callback) => {
                log::trace!("H3 stream {stream_id}: emitting {kind} priority \"{priority}\"");
                callback(stream_id, priority, is_update);
            }
            None => log::trace!(
                "H3 stream {stream_id}: {kind} priority \"{priority}\" parsed but no callback \
                 registered"
            ),
        }
    }

    /// Handle an RFC 9218 `PRIORITY_UPDATE` received on the peer's control stream. The
    /// prioritized element id must name a client-initiated bidirectional (request) stream —
    /// `id % 4 == 0` in QUIC — and other ids are ignored rather than erroring, since the signal
    /// is advisory.
    fn emit_priority_update(&self, prioritized_element_id: u64, priority: Priority) {
        if prioritized_element_id.is_multiple_of(4) {
            self.emit_priority(prioritized_element_id, priority, true);
        } else {
            log::trace!(
                "H3: ignoring PRIORITY_UPDATE for non-request stream {prioritized_element_id}"
            );
        }
    }

    /// Retrieve the [`Swansong`] shutdown handle for this HTTP/3 connection. See also
    /// [`H3Connection::shut_down`]
    pub fn swansong(&self) -> &Swansong {
        &self.swansong
    }

    /// Attempt graceful shutdown of this HTTP/3 connection (all streams).
    ///
    /// The returned [`ShutdownCompletion`] type can
    /// either be awaited in an async context or blocked on with [`ShutdownCompletion::block`] in a
    /// blocking context
    ///
    /// Note that this will NOT shut down the server. To shut down the whole server, use
    /// [`HttpContext::shut_down`]
    pub fn shut_down(&self) -> ShutdownCompletion {
        // Wake any in-flight `decode_field_section` calls parked on the decoder
        // table's `ThresholdWait` (a non-I/O future awaiting dynamic-table inserts
        // from the peer). The encoder table's writer loop is already swansong-
        // aware, but we mark it failed too for symmetry: any future state
        // mutations after shutdown are no longer wire-relevant.
        self.decoder_dynamic_table.fail(H3ErrorCode::NoError);
        self.encoder_dynamic_table.fail(H3ErrorCode::NoError);
        self.wake_peer_settings_waiters();
        self.swansong.shut_down()
    }

    /// Retrieve the [`HttpContext`] for this server.
    pub fn context(&self) -> Arc<HttpContext> {
        self.context.clone()
    }

    /// Returns the peer's HTTP/3 settings, available once the peer's control stream has been
    /// processed.
    pub fn peer_settings(&self) -> Option<&H3Settings> {
        self.peer_settings.get()
    }

    /// Record that we accepted a bidirectional stream with this ID.
    fn record_accepted_stream(&self, stream_id: u64) {
        self.max_accepted_stream_id
            .fetch_max(stream_id, Ordering::Relaxed);
        self.has_accepted_stream.store(true, Ordering::Relaxed);
    }

    /// The stream ID to send in a GOAWAY frame: one past the highest stream we accepted, or 0 if we
    /// haven't accepted any.
    fn goaway_id(&self) -> u64 {
        if self.has_accepted_stream.load(Ordering::Relaxed) {
            self.max_accepted_stream_id.load(Ordering::Relaxed) + 4
        } else {
            0
        }
    }

    /// Begin processing a single HTTP/3 request-response cycle on an accepted bidirectional
    /// stream.
    ///
    /// Returns a builder. Attach an optional reset hook with
    /// [`with_reset`][H3BidiRequest::with_reset], then `.await` it to run one request/response
    /// cycle. Awaiting resolves to [`H3StreamResult::WebTransport`] if the stream opens a
    /// WebTransport session rather than a standard HTTP/3 request.
    ///
    /// Without a reset hook, a stream-level protocol error drops the transport without
    /// resetting it; attach `with_reset` to issue the RST that RFC 9114 requires for stream
    /// errors.
    ///
    /// RFC 9218 priority is delivered out of band via the callback registered with
    /// [`register_priority_callback`][Self::register_priority_callback]: this method emits the
    /// request's initial priority once the headers are parsed.
    pub fn process_inbound_bidi<Transport, Handler>(
        self: Arc<Self>,
        transport: Transport,
        handler: Handler,
        stream_id: u64,
    ) -> H3BidiRequest<Transport, Handler> {
        H3BidiRequest {
            h3: self,
            transport,
            handler,
            stream_id,
            reset: None,
            reject_requests: false,
        }
    }

    /// Process a single HTTP/3 request-response cycle on a bidirectional stream, calling
    /// `reset` to issue a stream RST when a stream-level protocol error occurs.
    ///
    /// On any `H3Error::Protocol(code)` produced by first-frame processing (HEADERS decode,
    /// pseudo-header validation, etc.), `reset` is invoked with the still-owned transport and
    /// the error code before the error is returned. This lets callers RST both the recv and
    /// send halves of the bidi stream — required by RFC 9114 for stream errors like
    /// `H3_MESSAGE_ERROR`. I/O errors and successful runs do not invoke `reset`.
    ///
    /// `reset` is a `FnOnce` taking `(&mut Transport, H3ErrorCode)`. trillium-http does not
    /// itself depend on any reset capability of the transport; callers wire up the actual
    /// stream-RST mechanism (e.g. quinn's `RecvStream::stop` + `SendStream::reset`) inside
    /// the closure.
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` in case of io error or http/3 semantic error.
    // This is not deprecated yet because it didn't make sense to release a new version of
    // trillium-client just to avoid this deprecation, but the intention is to deprecate
    pub async fn process_inbound_bidi_with_reset<Transport, Handler, Fut, Reset>(
        self: Arc<Self>,
        mut transport: Transport,
        handler: Handler,
        stream_id: u64,
        reset: Reset,
    ) -> Result<H3StreamResult<Transport>, H3Error>
    where
        Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
        Handler: FnOnce(Conn<Transport>) -> Fut,
        Fut: Future<Output = Conn<Transport>>,
        Reset: FnOnce(&mut Transport, H3ErrorCode),
    {
        self.record_accepted_stream(stream_id);
        let _guard = self.swansong.guard();
        let mut buffer: Buffer =
            Vec::with_capacity(self.context.config.request_buffer_initial_len).into();

        let outcome =
            Conn::process_first_frame_h3(&self, &mut transport, &mut buffer, stream_id).await;

        match outcome {
            Ok(H3FirstFrame::Request {
                validated,
                start_time,
            }) => {
                let conn =
                    Conn::build_h3(self, transport, buffer, validated, start_time, stream_id);
                Ok(H3StreamResult::Request(
                    handler(conn).await.send_h3().await?,
                ))
            }
            Ok(H3FirstFrame::WebTransport { session_id }) => Ok(H3StreamResult::WebTransport {
                session_id,
                transport,
                buffer,
            }),
            Err(error) => {
                if let H3Error::Protocol(code) = &error {
                    reset(&mut transport, *code);
                }
                Err(error)
            }
        }
    }

    /// Decode a QPACK-encoded field section, consulting the dynamic table as needed.
    ///
    /// If the field section's Required Insert Count is greater than zero, waits until the
    /// dynamic table has received enough entries. Returns an error on protocol violations or
    /// if the encoder stream fails while waiting.
    ///
    /// Duplicate pseudo-headers are silently ignored (first value wins). Unknown
    /// pseudo-headers are rejected.
    ///
    /// # Errors
    ///
    /// Returns an error if the encoded bytes cannot be parsed as a valid field section.
    #[cfg(feature = "unstable")]
    pub async fn decode_field_section(
        &self,
        encoded: &[u8],
        stream_id: u64,
    ) -> Result<FieldSection<'static>, H3Error> {
        self.decoder_dynamic_table.decode(encoded, stream_id).await
    }

    #[cfg(not(feature = "unstable"))]
    pub(crate) async fn decode_field_section(
        &self,
        encoded: &[u8],
        stream_id: u64,
    ) -> Result<FieldSection<'static>, H3Error> {
        self.decoder_dynamic_table.decode(encoded, stream_id).await
    }

    /// Encode a QPACK field section (no HTTP/3 framing) from pseudo-headers and headers,
    /// consulting the encoder dynamic table to emit literal-with-name-reference or indexed
    /// representations as the table's contents allow.
    ///
    /// Superseded by [`encode_field_section_framed`][Self::encode_field_section_framed], which
    /// also frames the section and enforces the peer's field-section size limit.
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` in case of http/3 semantic error.
    // Retained only so an older `trillium-client` that predates the framed method still builds
    // against this crate; remove it at the next breaking release.
    #[cfg(feature = "unstable")]
    #[allow(clippy::unnecessary_wraps, reason = "future-proofing api")]
    pub fn encode_field_section(
        &self,
        field_section: &FieldSection<'_>,
        buf: &mut Vec<u8>,
        stream_id: u64,
    ) -> Result<(), H3Error> {
        self.encoder_dynamic_table
            .encode(field_section, buf, stream_id);
        Ok(())
    }

    /// Encode `field_section` as a complete HTTP/3 HEADERS frame — the QPACK-compressed field
    /// section prefixed with its `type` + `length` frame header — and append it to `buffer`.
    ///
    /// If the peer's `SETTINGS_MAX_FIELD_SECTION_SIZE` is known and the section's
    /// [`uncompressed_len`][crate::headers::FieldSection] exceeds it, the section is rejected
    /// before encoding and nothing is written. Until the peer's SETTINGS arrive the limit is
    /// unknown and unenforced (RFC 9114 §4.2.2).
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::InvalidData`] if the field section exceeds the peer's advertised limit, or
    /// the QPACK encoder's error mapped through [`io::Error::other`].
    #[cfg(feature = "unstable")]
    #[doc(hidden)]
    pub fn encode_field_section_framed(
        &self,
        field_section: &FieldSection<'_>,
        buffer: &mut Vec<u8>,
        stream_id: u64,
    ) -> io::Result<()> {
        self.encode_field_section_framed_impl(field_section, buffer, stream_id)
    }

    #[cfg(not(feature = "unstable"))]
    pub(crate) fn encode_field_section_framed(
        &self,
        field_section: &FieldSection<'_>,
        buffer: &mut Vec<u8>,
        stream_id: u64,
    ) -> io::Result<()> {
        self.encode_field_section_framed_impl(field_section, buffer, stream_id)
    }

    fn encode_field_section_framed_impl(
        &self,
        field_section: &FieldSection<'_>,
        buffer: &mut Vec<u8>,
        stream_id: u64,
    ) -> io::Result<()> {
        // Enforce the peer's SETTINGS_MAX_FIELD_SECTION_SIZE against the uncompressed size the
        // limit is defined in (RFC 9114 §4.2.2), before encoding — an over-limit section is
        // rejected without being written to the wire.
        if let Some(max_size) = self
            .peer_settings()
            .and_then(H3Settings::max_field_section_size)
        {
            let size = field_section.uncompressed_len();
            if size > max_size {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    format!("field section would be longer than peer allows ({size} > {max_size})"),
                ));
            }
        }

        let start = buffer.len();
        self.encoder_dynamic_table
            .encode(field_section, buffer, stream_id);

        // The HEADERS frame length is the encoded (compressed) payload length, distinct from the
        // uncompressed size enforced above. Encode the section in place, then open a gap in front
        // of it for the now-known frame header and shift the section right into place.
        let section_len = buffer.len() - start;
        let frame = Frame::Headers(section_len as u64);
        let frame_header_len = frame.encoded_len();
        buffer.resize(buffer.len() + frame_header_len, 0);
        buffer.copy_within(start..start + section_len, start + frame_header_len);
        frame.encode(&mut buffer[start..start + frame_header_len]);
        Ok(())
    }

    /// Run this connection's HTTP/3 outbound control stream.
    ///
    /// Sends the initial SETTINGS frame, then sends GOAWAY when the connection shuts down.
    /// Returns after GOAWAY is sent; keep the stream open until the QUIC connection closes
    /// (closing a control stream is a connection error).
    ///
    /// Shuts the connection down ([`shut_down`][Self::shut_down]) on return, for the same reason
    /// as [`run_encoder`][Self::run_encoder].
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` in case of io error or http/3 semantic error.
    pub async fn run_outbound_control<T>(&self, mut stream: T) -> Result<(), H3Error>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let result: Result<(), H3Error> = async {
            let mut buf = vec![0; 128];

            let settings = Frame::Settings(H3Settings::from(&self.context.config));
            log::trace!(
                "H3 outbound control: sending SETTINGS: {:?}",
                H3Settings::from(&self.context.config)
            );

            write(&mut buf, &mut stream, |buf| {
                let mut written = quic_varint::encode(UniStreamType::Control, buf)?;
                written += settings.encode(&mut buf[written..])?;
                Some(written)
            })
            .await?;
            log::trace!("H3 outbound control: SETTINGS sent");

            self.swansong.clone().await;

            write(&mut buf, &mut stream, |buf| {
                Frame::Goaway(self.goaway_id()).encode(buf)
            })
            .await?;

            Ok(())
        }
        .await;

        self.shut_down();
        result
    }

    /// Run the outbound QPACK encoder stream for the duration of the connection.
    ///
    /// Writes the stream type byte, then drains encoder-stream instructions from the encoder
    /// dynamic table as they are enqueued. Returns when the connection shuts down or the table is
    /// marked failed.
    ///
    /// Shuts the connection down ([`shut_down`][Self::shut_down]) on return. This stream is
    /// mandatory for the connection's lifetime, so its termination — clean or errored — means the
    /// connection can no longer function; marking it shut down lets a pooling caller evict it
    /// rather than hand it back out. Idempotent on the clean path, which returns only after
    /// shutdown has already begun.
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` in case of io error.
    pub async fn run_encoder<T>(&self, mut stream: T) -> Result<(), H3Error>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let result = self
            .encoder_dynamic_table
            .run_writer(&mut stream, self.swansong.clone())
            .await;
        self.shut_down();
        result
    }

    /// Run the outbound QPACK decoder stream for the duration of the connection.
    ///
    /// Writes the stream type byte, then loops sending Section Acknowledgement and Insert
    /// Count Increment instructions as they become needed. Returns when the connection
    /// shuts down.
    ///
    /// Shuts the connection down ([`shut_down`][Self::shut_down]) on return, for the same reason
    /// as [`run_encoder`][Self::run_encoder].
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` in case of io error or http/3 semantic error.
    pub async fn run_decoder<T>(&self, mut stream: T) -> Result<(), H3Error>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let result = self
            .decoder_dynamic_table
            .run_writer(&mut stream, self.swansong.clone())
            .await;
        self.shut_down();
        result
    }

    /// Handle an inbound unidirectional HTTP/3 stream from the peer.
    ///
    /// Internal stream types (control, QPACK encoder/decoder) are handled automatically;
    /// application streams are returned via [`UniStreamResult`] for the caller to process.
    ///
    /// On a connection-level protocol error, this method drops the recv stream before
    /// the caller can react. Quinn's `RecvStream::drop` then sends `STOP_SENDING`, which
    /// races against the caller's `connection.close` — if the peer responds with a
    /// malformed `RESET_STREAM` (notably `final_offset = 0`) before our app close is
    /// applied, the transport-level error overrides our app error code on the wire.
    /// Use [`process_inbound_uni_with_close`] to thread the close call through the
    /// function so it fires before the stream drops.
    ///
    /// [`process_inbound_uni_with_close`]: Self::process_inbound_uni_with_close
    ///
    /// # Errors
    ///
    /// Returns a `H3Error` in case of io error or http/3 semantic error.
    #[deprecated(
        since = "1.2.0",
        note = "use `process_inbound_uni_with_close` so connection-level protocol errors close \
                the QUIC connection before the recv stream drops, avoiding a `FINAL_SIZE_ERROR` \
                race with the peer's response to STOP_SENDING"
    )]
    pub async fn process_inbound_uni<T>(&self, stream: T) -> Result<UniStreamResult<T>, H3Error>
    where
        T: AsyncRead + Unpin + Send,
    {
        self.process_inbound_uni_with_close(stream, |_| {}).await
    }

    /// Handle an inbound unidirectional HTTP/3 stream from the peer, calling `on_close` to
    /// close the QUIC connection if a connection-level protocol error is detected.
    ///
    /// Identical to [`process_inbound_uni`][Self::process_inbound_uni] except that on
    /// any `H3Error::Protocol(code)` whose code is a connection-level error (RFC 9114,
    /// RFC 9204), `on_close` is invoked with that code while the recv stream is still alive. This
    /// lets callers send a `CONNECTION_CLOSE` before the stream drops — if the close call sets
    /// quinn's `conn.error`, quinn's `RecvStream::drop` skips `STOP_SENDING`, eliminating a
    /// peer race that otherwise causes `FINAL_SIZE_ERROR` to override the app error code.
    ///
    /// `on_close` is a `FnOnce` taking `H3ErrorCode`. trillium-http does not itself
    /// hold the QUIC connection; callers wire up the actual `connection.close()` call
    /// inside the closure (e.g. quinn's `Connection::close`).
    ///
    /// # Errors
    ///
    /// Returns a `H3Error` in case of io error or http/3 semantic error.
    pub async fn process_inbound_uni_with_close<T, OnClose>(
        &self,
        mut stream: T,
        on_close: OnClose,
    ) -> Result<UniStreamResult<T>, H3Error>
    where
        T: AsyncRead + Unpin + Send,
        OnClose: FnOnce(H3ErrorCode),
    {
        let inner = self
            .swansong
            .interrupt(self.process_inbound_uni_inner(&mut stream))
            .await
            .unwrap_or(Ok(UniInnerResult::Handled)); // interrupted

        match inner {
            Ok(UniInnerResult::Handled) => Ok(UniStreamResult::Handled),
            Ok(UniInnerResult::WebTransport { session_id, buffer }) => {
                Ok(UniStreamResult::WebTransport {
                    session_id,
                    stream,
                    buffer,
                })
            }
            Ok(UniInnerResult::Unknown { stream_type }) => Ok(UniStreamResult::Unknown {
                stream_type,
                stream,
            }),
            Err(error) => {
                // Fire `on_close` BEFORE returning so the caller's connection.close
                // call sets quinn's `conn.error` while `stream` is still alive. When
                // `stream` then drops at function return, quinn's `RecvStream::drop`
                // skips STOP_SENDING — preventing the peer-RESET_STREAM race that
                // otherwise replaces our app close code with FINAL_SIZE_ERROR.
                if let H3Error::Protocol(code) = &error
                    && code.is_connection_error()
                {
                    on_close(*code);
                }
                Err(error)
            }
        }
    }

    /// Inner-loop body of [`process_inbound_uni_with_close`][Self::process_inbound_uni_with_close].
    /// Borrows `stream` so the outer function can keep ownership of it across the await,
    /// which lets the caller's close callback fire before the recv stream drops.
    async fn process_inbound_uni_inner<T>(&self, stream: &mut T) -> Result<UniInnerResult, H3Error>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut buf = vec![0; 128];
        let mut filled = 0;

        // Read stream type varint (decode as raw u64 to handle unknown types)
        let stream_type = read(&mut buf, &mut filled, stream, |data| {
            match quic_varint::decode(data) {
                Ok(ok) => Ok(Some(ok)),
                Err(QuicVarIntError::UnexpectedEnd) => Ok(None),
                // this branch is unreachable because u64 is always From<u64>
                Err(QuicVarIntError::UnknownValue { bytes, value }) => Ok(Some((value, bytes))),
            }
        })
        .await?;

        match UniStreamType::try_from(stream_type) {
            Ok(UniStreamType::Control) => {
                log::trace!("H3 inbound uni: control stream");
                self.run_inbound_control(&mut buf, &mut filled, stream)
                    .await?;
                Ok(UniInnerResult::Handled)
            }

            Ok(UniStreamType::QpackEncoder) => {
                log::trace!("H3 inbound uni: QPACK encoder stream ({filled} bytes pre-read)");
                let mut reader = Prepended {
                    head: &buf[..filled],
                    tail: stream,
                };

                log::trace!("QPACK encoder stream: started");
                self.decoder_dynamic_table.run_reader(&mut reader).await?;

                Ok(UniInnerResult::Handled)
            }

            Ok(UniStreamType::QpackDecoder) => {
                log::trace!("H3 inbound uni: QPACK decoder stream ({filled} bytes pre-read)");
                let mut reader = Prepended {
                    head: &buf[..filled],
                    tail: stream,
                };
                self.encoder_dynamic_table.run_reader(&mut reader).await?;
                Ok(UniInnerResult::Handled)
            }

            Ok(UniStreamType::WebTransport) => {
                log::trace!("H3 inbound uni: WebTransport stream");
                let session_id =
                    read(
                        &mut buf,
                        &mut filled,
                        stream,
                        |data| match quic_varint::decode(data) {
                            Ok(ok) => Ok(Some(ok)),
                            Err(QuicVarIntError::UnexpectedEnd) => Ok(None),
                            Err(QuicVarIntError::UnknownValue { bytes, value }) => {
                                Ok(Some((value, bytes)))
                            }
                        },
                    )
                    .await?;

                buf.truncate(filled);

                Ok(UniInnerResult::WebTransport {
                    session_id,
                    buffer: buf.into(),
                })
            }

            Ok(UniStreamType::Push) => {
                // Trillium does not support HTTP/3 push, so we hand these back as `Unknown`
                // identically to truly-unknown stream types — the explicit arm exists so
                // trace output names "push stream" rather than a bare type id.
                log::trace!("H3 inbound uni: push stream (push not supported)");
                Ok(UniInnerResult::Unknown { stream_type })
            }

            Err(_) => {
                log::trace!("H3 inbound uni: unknown stream type {stream_type:#x}");
                Ok(UniInnerResult::Unknown { stream_type })
            }
        }
    }

    /// Handle the http/3 peer's inbound control stream.
    ///
    /// # Errors
    ///
    /// Returns a `H3Error` in case of io error or HTTP/3 semantic error.
    async fn run_inbound_control<T>(
        &self,
        buf: &mut Vec<u8>,
        filled: &mut usize,
        stream: &mut T,
    ) -> Result<(), H3Error>
    where
        T: AsyncRead + Unpin + Send,
    {
        // SettingsError takes priority: a SETTINGS frame whose payload is itself invalid
        // (e.g. forbidden HTTP/2 setting IDs) is reported as SETTINGS_ERROR, not the
        // MISSING_SETTINGS we report for everything else here.
        let settings = read(buf, filled, stream, |data| match Frame::decode(data) {
            Ok((Frame::Settings(s), consumed)) => Ok(Some((s, consumed))),
            Err(FrameDecodeError::Incomplete) => Ok(None),
            Err(FrameDecodeError::Error(H3ErrorCode::SettingsError)) => {
                Err(H3ErrorCode::SettingsError)
            }
            Ok(_) | Err(FrameDecodeError::Error(_)) => Err(H3ErrorCode::MissingSettings),
        })
        .await
        .map_err(map_critical_stream_eof)?;

        log::trace!("H3 peer settings: {settings:?}");

        self.peer_settings
            .set(settings)
            .map_err(|_| H3ErrorCode::FrameUnexpected)?;
        self.wake_peer_settings_waiters();

        self.encoder_dynamic_table
            .initialize_from_peer_settings(settings);

        loop {
            let frame = self
                .swansong
                .interrupt(read(buf, filled, stream, |data| {
                    match Frame::decode(data) {
                        Ok((frame, consumed)) => Ok(Some((frame, consumed))),
                        Err(FrameDecodeError::Incomplete) => Ok(None),
                        Err(FrameDecodeError::Error(code)) => Err(code),
                    }
                }))
                .await
                .transpose()
                .map_err(map_critical_stream_eof)?;

            match frame {
                None => {
                    log::trace!("H3 control stream: interrupted by shutdown");
                    return Ok(());
                }

                Some(Frame::Goaway(id)) => {
                    log::trace!("H3 control stream: peer sent GOAWAY(stream_id={id})");
                    self.swansong.shut_down();
                    return Ok(());
                }

                Some(Frame::Unknown(n)) => {
                    // Consume the payload bytes so the stream stays synchronized.
                    log::trace!("H3 control stream: skipping unknown frame (payload {n} bytes)");
                    let n = usize::try_from(n).unwrap_or(usize::MAX);
                    let in_buf = n.min(*filled);
                    buf.copy_within(in_buf..*filled, 0);
                    *filled -= in_buf;
                    let mut todo = n - in_buf;
                    let mut scratch = [0u8; 256];
                    while todo > 0 {
                        let to_read = todo.min(scratch.len());
                        let n = stream
                            .read(&mut scratch[..to_read])
                            .await
                            .map_err(H3Error::Io)?;
                        if n == 0 {
                            return Err(H3ErrorCode::ClosedCriticalStream.into());
                        }
                        todo -= n;
                    }
                }

                Some(
                    Frame::Settings(_)
                    | Frame::Data(_)
                    | Frame::Headers(_)
                    | Frame::PushPromise { .. }
                    | Frame::WebTransport(_),
                ) => {
                    return Err(H3ErrorCode::FrameUnexpected.into());
                }

                Some(Frame::PriorityUpdate {
                    prioritized_element_id,
                    priority,
                }) => {
                    log::trace!(
                        "H3 control stream: PRIORITY_UPDATE stream={prioritized_element_id} \
                         priority=\"{priority}\""
                    );
                    self.emit_priority_update(prioritized_element_id, priority);
                }

                // Trillium doesn't implement push, so these are ignored rather than acted on.
                Some(Frame::CancelPush(_) | Frame::MaxPushId(_)) => {
                    log::trace!("H3 control stream: ignoring {frame:?}");
                }
            }
        }
    }
}

/// A pending HTTP/3 request-response cycle on one bidirectional stream, with optional
/// per-stream hooks.
///
/// Built by [`H3Connection::process_inbound_bidi`]. Configure hooks with the `with_*`
/// methods and `.await` it to run the cycle. New per-stream extension points are added as
/// further `with_*` methods, so the entry point's required arguments never change.
pub struct H3BidiRequest<Transport, Handler> {
    h3: Arc<H3Connection>,
    transport: Transport,
    handler: Handler,
    stream_id: u64,
    reset: Option<ResetHook<Transport>>,
    reject_requests: bool,
}

/// Per-stream reset hook: RST both halves with the still-owned transport on a stream-level error.
type ResetHook<Transport> = Box<dyn FnOnce(&mut Transport, H3ErrorCode) + Send>;

impl<Transport, Handler> std::fmt::Debug for H3BidiRequest<Transport, Handler> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("H3BidiRequest")
            .field("stream_id", &self.stream_id)
            .finish_non_exhaustive()
    }
}

impl<Transport, Handler> H3BidiRequest<Transport, Handler> {
    /// Issue a stream RST on a stream-level protocol error.
    ///
    /// On any `H3Error::Protocol(code)` from first-frame processing, `reset` is called with
    /// the still-owned transport and the error code before the error is returned — letting the
    /// caller RST both halves of the bidi stream as RFC 9114 requires. I/O errors and
    /// successful runs do not invoke it. Without this hook, the transport is dropped without a
    /// reset.
    #[must_use]
    pub fn with_reset<R>(mut self, reset: R) -> Self
    where
        R: FnOnce(&mut Transport, H3ErrorCode) + Send + 'static,
    {
        self.reset = Some(Box::new(reset));
        self
    }

    /// Treat an HTTP request on this stream as a protocol violation instead of running the
    /// handler.
    ///
    /// A peer that accepts inbound bidirectional streams only for negotiated extensions —
    /// an HTTP/3 client, where RFC 9114 makes a server-initiated request stream a connection
    /// error — enables this to refuse requests without responding to them. When the stream's
    /// first frame begins an HTTP request, the handler is skipped, nothing is written to the
    /// stream, the [`with_reset`][Self::with_reset] hook is invoked with
    /// [`H3ErrorCode::StreamCreationError`], and awaiting resolves to that error. Closing the
    /// connection is the caller's responsibility, typically inside the reset hook while the
    /// stream is still alive.
    #[must_use]
    pub fn with_request_rejection(mut self) -> Self {
        self.reject_requests = true;
        self
    }
}

impl<Transport, Handler, Fut> IntoFuture for H3BidiRequest<Transport, Handler>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    Handler: FnOnce(Conn<Transport>) -> Fut + Send + 'static,
    Fut: Future<Output = Conn<Transport>> + Send + 'static,
{
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;
    type Output = Result<H3StreamResult<Transport>, H3Error>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let Self {
                h3,
                mut transport,
                handler,
                stream_id,
                reset,
                reject_requests,
            } = self;

            h3.record_accepted_stream(stream_id);
            let _guard = h3.swansong.guard();
            let mut buffer: Buffer =
                Vec::with_capacity(h3.context.config.request_buffer_initial_len).into();

            let outcome =
                Conn::process_first_frame_h3(&h3, &mut transport, &mut buffer, stream_id).await;

            match outcome {
                Ok(H3FirstFrame::Request { .. }) if reject_requests => {
                    let code = H3ErrorCode::StreamCreationError;
                    if let Some(reset) = reset {
                        reset(&mut transport, code);
                    }
                    Err(code.into())
                }
                Ok(H3FirstFrame::Request {
                    validated,
                    start_time,
                }) => {
                    let initial_priority = validated
                        .request_headers
                        .get_str(KnownHeaderName::Priority)
                        .map(Priority::parse)
                        .unwrap_or_default();
                    h3.emit_priority(stream_id, initial_priority, false);
                    let conn =
                        Conn::build_h3(h3, transport, buffer, validated, start_time, stream_id);
                    Ok(H3StreamResult::Request(
                        handler(conn).await.send_h3().await?,
                    ))
                }
                Ok(H3FirstFrame::WebTransport { session_id }) => Ok(H3StreamResult::WebTransport {
                    session_id,
                    transport,
                    buffer,
                }),
                Err(error) => {
                    if let H3Error::Protocol(code) = &error
                        && let Some(reset) = reset
                    {
                        reset(&mut transport, *code);
                    }
                    Err(error)
                }
            }
        })
    }
}

/// Map an `UnexpectedEof` I/O error (the `read` helper's "stream FIN'd" signal) to
/// `H3_CLOSED_CRITICAL_STREAM`. Closure of the control stream or of either QPACK
/// side-channel is a connection error. Other I/O errors and any protocol error are passed
/// through unchanged.
fn map_critical_stream_eof(error: H3Error) -> H3Error {
    match error {
        H3Error::Io(e) if e.kind() == ErrorKind::UnexpectedEof => {
            H3ErrorCode::ClosedCriticalStream.into()
        }
        other => other,
    }
}

async fn write(
    buf: &mut Vec<u8>,
    mut stream: impl AsyncWrite + Unpin + Send,
    mut f: impl FnMut(&mut [u8]) -> Option<usize>,
) -> io::Result<usize> {
    let written = loop {
        if let Some(w) = f(buf) {
            break w;
        }
        if buf.len() >= MAX_BUFFER_SIZE {
            return Err(io::Error::new(ErrorKind::OutOfMemory, "runaway allocation"));
        }
        buf.resize(buf.len() * 2, 0);
    };

    stream.write_all(&buf[..written]).await?;
    stream.flush().await?;
    Ok(written)
}

/// An `AsyncRead` adapter that drains a byte slice before reading from an inner stream.
///
/// Used to replay bytes that were read ahead while parsing a stream-type varint, before
/// dispatching to the inner runner that consumes the rest of the stream.
struct Prepended<'a, T> {
    head: &'a [u8],
    tail: T,
}

impl<T: AsyncRead + Unpin> AsyncRead for Prepended<'_, T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if !this.head.is_empty() {
            let n = this.head.len().min(out.len());
            out[..n].copy_from_slice(&this.head[..n]);
            this.head = &this.head[n..];
            return Poll::Ready(Ok(n));
        }
        Pin::new(&mut this.tail).poll_read(cx, out)
    }
}

/// Read from `stream` into `buf` until `f` can decode a value.
///
/// `f` receives the filled portion of the buffer and returns:
/// - `Ok(Some((value, consumed)))` — success; consumed bytes are removed from the front
/// - `Ok(None)` — need more data; reads more bytes and retries
/// - `Err(e)` — unrecoverable error; propagated to caller
async fn read<R>(
    buf: &mut Vec<u8>,
    filled: &mut usize,
    stream: &mut (impl AsyncRead + Unpin + Send),
    f: impl Fn(&[u8]) -> Result<Option<(R, usize)>, H3ErrorCode>,
) -> Result<R, H3Error> {
    loop {
        if let Some((result, consumed)) = f(&buf[..*filled])? {
            buf.copy_within(consumed..*filled, 0);
            *filled -= consumed;
            return Ok(result);
        }

        if *filled >= buf.len() {
            if buf.len() >= MAX_BUFFER_SIZE {
                return Err(io::Error::new(ErrorKind::OutOfMemory, "runaway allocation").into());
            }
            buf.resize(buf.len() * 2, 0);
        }

        let n = stream.read(&mut buf[*filled..]).await?;
        if n == 0 {
            return Err(io::Error::new(ErrorKind::UnexpectedEof, "stream closed").into());
        }
        *filled += n;
    }
}

#[cfg(test)]
mod tests;
