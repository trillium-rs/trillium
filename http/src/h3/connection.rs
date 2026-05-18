mod peer_settings_wait;

use super::{
    H3Error,
    frame::{Frame, FrameDecodeError, UniStreamType},
    quic_varint::{self, QuicVarIntError},
    settings::H3Settings,
};
use crate::{
    Buffer, Conn, HttpContext,
    conn::H3FirstFrame,
    h3::{H3ErrorCode, MAX_BUFFER_SIZE},
    headers::qpack::{DecoderDynamicTable, EncoderDynamicTable, FieldSection},
};
use event_listener::Event;
use futures_lite::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use std::{
    future::Future,
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
        })
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

    /// Process a single HTTP/3 request-response cycle on a bidirectional stream.
    ///
    /// Call this once per accepted bidirectional stream. Returns
    /// [`H3StreamResult::WebTransport`] if the stream opens a WebTransport session rather than
    /// a standard HTTP/3 request.
    ///
    /// On a stream-level protocol error (e.g. malformed pseudo-headers,
    /// `H3_MESSAGE_ERROR`), this method drops the transport without resetting it. To honour
    /// RFC 9114's stream-error MUSTs, callers should use [`process_inbound_bidi_with_reset`]
    /// instead and pass a closure that issues a stream RST with the protocol error code.
    ///
    /// [`process_inbound_bidi_with_reset`]: Self::process_inbound_bidi_with_reset
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` in case of io error or http/3 semantic error.
    #[deprecated(
        since = "1.2.0",
        note = "use `process_inbound_bidi_with_reset` so stream-level protocol errors RST the \
                stream as required by RFC 9114"
    )]
    pub async fn process_inbound_bidi<Transport, Handler, Fut>(
        self: Arc<Self>,
        transport: Transport,
        handler: Handler,
        stream_id: u64,
    ) -> Result<H3StreamResult<Transport>, H3Error>
    where
        Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
        Handler: FnOnce(Conn<Transport>) -> Fut,
        Fut: Future<Output = Conn<Transport>>,
    {
        self.process_inbound_bidi_with_reset(transport, handler, stream_id, |_, _| {})
            .await
    }

    /// Process a single HTTP/3 request-response cycle on a bidirectional stream, calling
    /// `reset` to issue a stream RST when a stream-level protocol error occurs.
    ///
    /// Identical to [`process_inbound_bidi`][Self::process_inbound_bidi] except that on any
    /// `H3Error::Protocol(code)` produced by first-frame processing (HEADERS decode,
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

    /// Encode a QPACK field section from pseudo-headers and headers, consulting the encoder
    /// dynamic table to emit literal-with-name-reference or indexed representations as the
    /// table's contents allow.
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` in case of http/3 semantic error.
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

    #[cfg(not(feature = "unstable"))]
    #[allow(clippy::unnecessary_wraps, reason = "future-proofing api")]
    pub(crate) fn encode_field_section(
        &self,
        field_section: &FieldSection<'_>,
        buf: &mut Vec<u8>,
        stream_id: u64,
    ) -> Result<(), H3Error> {
        self.encoder_dynamic_table
            .encode(field_section, buf, stream_id);
        Ok(())
    }

    /// Run this connection's HTTP/3 outbound control stream.
    ///
    /// Sends the initial SETTINGS frame, then sends GOAWAY when the connection shuts down.
    /// Returns after GOAWAY is sent; keep the stream open until the QUIC connection closes
    /// (closing a control stream is a connection error).
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` in case of io error or http/3 semantic error.
    pub async fn run_outbound_control<T>(&self, mut stream: T) -> Result<(), H3Error>
    where
        T: AsyncWrite + Unpin + Send,
    {
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

    /// Run the outbound QPACK encoder stream for the duration of the connection.
    ///
    /// Writes the stream type byte, then drains encoder-stream instructions from the encoder
    /// dynamic table as they are enqueued. Returns when the connection shuts down or the table is
    /// marked failed.
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` in case of io error.
    pub async fn run_encoder<T>(&self, mut stream: T) -> Result<(), H3Error>
    where
        T: AsyncWrite + Unpin + Send,
    {
        self.encoder_dynamic_table
            .run_writer(&mut stream, self.swansong.clone())
            .await
    }

    /// Run the outbound QPACK decoder stream for the duration of the connection.
    ///
    /// Writes the stream type byte, then loops sending Section Acknowledgement and Insert
    /// Count Increment instructions as they become needed. Returns when the connection
    /// shuts down.
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` in case of io error or http/3 semantic error.
    pub async fn run_decoder<T>(&self, mut stream: T) -> Result<(), H3Error>
    where
        T: AsyncWrite + Unpin + Send,
    {
        self.decoder_dynamic_table
            .run_writer(&mut stream, self.swansong.clone())
            .await
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

                // Trillium doesn't implement push, so these are ignored rather than acted on.
                Some(Frame::CancelPush(_) | Frame::MaxPushId(_)) => {
                    log::trace!("H3 control stream: ignoring {frame:?}");
                }
            }
        }
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
