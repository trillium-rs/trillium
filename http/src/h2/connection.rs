//! HTTP/2 connection driver (RFC 9113).
//!
//! [`H2Connection`] is the shared, [`Arc`]-able per-connection state — handler tasks reference it
//! by way of their [`H2Transport`] to talk back to the driver. [`H2Acceptor`] owns the underlying
//! TCP transport and the demux state, and is driven by the runtime adapter via
//! [`H2Acceptor::next`]: each call returns the next opened request stream (an [`H2Transport`] for
//! the runtime to spawn a handler task against), or `None` when the connection is closed.
//!
//! Phase 3 (in progress): the acceptor handshakes, reassembles HEADERS + CONTINUATION blocks,
//! decodes them via HPACK, and emits each new request stream as an [`H2Transport`]. DATA frame
//! routing and send-side serialization land in subsequent commits.
//!
//! [`H2Transport`]: super::transport::H2Transport

use super::{
    H2Error, H2ErrorCode, H2Settings,
    frame::{self, FRAME_HEADER_LEN, Frame, FrameDecodeError, FrameHeader},
    transport::{H2Transport, StreamState},
};
use crate::{HttpContext, headers::hpack::HpackDecoder};
use futures_lite::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use std::{collections::HashMap, sync::Arc};
use swansong::{ShutdownCompletion, Swansong};

/// The client connection preface (RFC 9113 §3.4). 24 bytes the client MUST send before any
/// HTTP/2 frames.
pub(crate) const CLIENT_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// Upper bound for transient frame buffers — prevents runaway allocation on a peer that advertises
/// an absurd `MAX_FRAME_SIZE`. The per-connection maximum is negotiated via SETTINGS and will
/// replace this in phase 7.
const MAX_BUFFER_SIZE: usize = 1 << 20;

/// Initial HPACK dynamic table size per RFC 7541 §4.2 — also the value implied by an absent
/// `SETTINGS_HEADER_TABLE_SIZE`. Phase 7 will let `HttpConfig` raise or lower this; for now it's
/// hardcoded to match the default we advertise.
const HPACK_TABLE_SIZE: usize = 4096;

/// Shared per-connection state for HTTP/2.
///
/// Wrapped in an [`Arc`] and held by both the [`H2Acceptor`] driver and every [`H2Transport`]
/// handed to a handler task. Per-stream tables, HPACK encoder state, and connection-level send
/// flow control will accumulate here as later phases land.
///
/// [`H2Transport`]: super::transport::H2Transport
#[derive(Debug)]
pub struct H2Connection {
    context: Arc<HttpContext>,
    swansong: Swansong,
}

impl H2Connection {
    /// Construct a new `H2Connection` to manage HTTP/2 for a single peer.
    pub fn new(context: Arc<HttpContext>) -> Arc<Self> {
        let swansong = context.swansong().child();
        Arc::new(Self { context, swansong })
    }

    /// The [`HttpContext`] this connection was constructed with.
    pub fn context(&self) -> Arc<HttpContext> {
        self.context.clone()
    }

    /// The connection-scoped [`Swansong`]. Shuts down on peer GOAWAY or when the server-level
    /// swansong shuts down.
    pub fn swansong(&self) -> &Swansong {
        &self.swansong
    }

    /// Attempt graceful shutdown of this HTTP/2 connection.
    pub fn shut_down(&self) -> ShutdownCompletion {
        self.swansong.shut_down()
    }

    /// Bind this `H2Connection` to a TCP transport and return an [`H2Acceptor`] that drives the
    /// connection.
    ///
    /// The acceptor must be polled to completion via repeated calls to [`H2Acceptor::next`]; each
    /// returned [`H2Transport`] should be spawned on its own task.
    ///
    /// [`H2Transport`]: super::transport::H2Transport
    pub fn run<T>(self: Arc<Self>, transport: T) -> H2Acceptor<T>
    where
        T: AsyncRead + AsyncWrite + Unpin + Send,
    {
        H2Acceptor::new(self, transport)
    }
}

/// Owns the per-connection TCP transport and drives the HTTP/2 demux loop.
///
/// Created by [`H2Connection::run`]. The runtime adapter calls [`Self::next`] in a loop; each call
/// either returns the next opened request stream (an [`H2Transport`] to be spawned on a handler
/// task) or `None` when the connection is closed.
#[derive(Debug)]
pub struct H2Acceptor<T> {
    connection: Arc<H2Connection>,
    transport: T,
    state: AcceptorState,

    /// Reusable scratch buffer for the next frame's header + payload.
    read_buf: Vec<u8>,

    /// Set to `true` when the driver has emitted a final GOAWAY (graceful or `PROTOCOL_ERROR`)
    /// and `Self::next` should subsequently return `Ok(None)`.
    finished: bool,

    /// HPACK decoder state, shared across all header blocks on this connection.
    hpack: HpackDecoder,

    /// Per-stream state, keyed by stream id. Driver-only — handler tasks hold their own
    /// `Arc<StreamState>` via [`H2Transport`] and don't consult this table.
    streams: HashMap<u32, Arc<StreamState>>,

    /// Highest peer-initiated stream id seen so far. Peer-initiated (client) stream ids must be
    /// odd and strictly increasing per RFC 9113 §5.1.1.
    last_peer_stream_id: u32,
}

/// Cursor through the connection setup → frame loop sequence inside [`H2Acceptor::next`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcceptorState {
    /// Haven't read the client preface yet.
    AwaitingPreface,
    /// Preface read; need to send our initial SETTINGS frame before processing peer frames.
    NeedsServerSettings,
    /// Steady state — read the next frame and dispatch.
    Running,
}

impl<T> H2Acceptor<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    fn new(connection: Arc<H2Connection>, transport: T) -> Self {
        Self {
            connection,
            transport,
            state: AcceptorState::AwaitingPreface,
            read_buf: vec![0u8; FRAME_HEADER_LEN],
            finished: false,
            hpack: HpackDecoder::new(HPACK_TABLE_SIZE),
            streams: HashMap::new(),
            last_peer_stream_id: 0,
        }
    }

    /// The shared [`H2Connection`] this acceptor was created from.
    pub fn connection(&self) -> &Arc<H2Connection> {
        &self.connection
    }

    /// Drive the connection until the next request stream opens, the connection ends, or a fatal
    /// protocol or I/O error occurs.
    ///
    /// Returns `Ok(Some(transport))` for each new request stream — the runtime adapter is expected
    /// to spawn a handler task that consumes the [`H2Transport`]. Returns `Ok(None)` when the
    /// connection has been shut down cleanly (peer GOAWAY, our own swansong shutdown, or graceful
    /// peer close).
    ///
    /// # Errors
    ///
    /// Returns an [`H2Error`] for any protocol violation detected while decoding peer frames or
    /// for an unrecoverable transport I/O error. A final GOAWAY is sent before this method returns
    /// (best-effort; I/O errors skip it).
    pub async fn next(&mut self) -> Result<Option<H2Transport>, H2Error> {
        if self.finished {
            return Ok(None);
        }

        // A successfully opened stream is the common path — return it without touching
        // `finished` so the next call resumes the loop. Anything else terminates the connection
        // (graceful or error) and is followed by a best-effort GOAWAY.
        match self.drive().await {
            Ok(Some(transport)) => Ok(Some(transport)),
            Ok(None) => {
                self.finished = true;
                let _ = send_goaway(&mut self.transport, 0, H2ErrorCode::NoError).await;
                Ok(None)
            }
            Err(H2Error::Protocol(code)) => {
                self.finished = true;
                let _ = send_goaway(&mut self.transport, 0, code).await;
                Err(H2Error::Protocol(code))
            }
            Err(e @ H2Error::Io(_)) => {
                self.finished = true;
                Err(e)
            }
        }
    }

    /// Inner loop body. Returns:
    /// - `Ok(Some(_))` when a new request stream has opened and should be returned to the caller.
    /// - `Ok(None)` on a clean connection shutdown (peer GOAWAY or local swansong).
    /// - `Err` on any protocol or I/O failure.
    async fn drive(&mut self) -> Result<Option<H2Transport>, H2Error> {
        if self.state == AcceptorState::AwaitingPreface {
            read_preface(&mut self.transport).await?;
            self.state = AcceptorState::NeedsServerSettings;
        }
        if self.state == AcceptorState::NeedsServerSettings {
            // Server-side preface per §3.4: our initial SETTINGS frame MUST be the first frame we
            // send.
            write_settings(&mut self.transport, &H2Settings::server_defaults()).await?;
            self.state = AcceptorState::Running;
        }

        loop {
            let read = self
                .connection
                .swansong
                .interrupt(read_frame(&mut self.transport, &mut self.read_buf))
                .await;

            let Some(decoded) = read else {
                return Ok(None);
            };
            let (frame, consumed) = decoded?;

            match frame {
                Frame::Settings(_) => write_settings_ack(&mut self.transport).await?,

                Frame::Ping {
                    opaque_data,
                    ack: false,
                } => write_ping_ack(&mut self.transport, opaque_data).await?,

                Frame::Goaway { .. } => {
                    self.connection.swansong.shut_down();
                    return Ok(None);
                }

                Frame::Headers {
                    stream_id,
                    end_stream,
                    end_headers,
                    header_block_length,
                    ..
                } => {
                    let transport = self
                        .open_stream(
                            stream_id,
                            end_stream,
                            end_headers,
                            header_block_length,
                            consumed,
                        )
                        .await?;
                    return Ok(Some(transport));
                }

                // §6.6 PUSH_PROMISE from a client is always a connection error; §6.10
                // CONTINUATION outside an in-progress header block is too. DATA without
                // stream-machinery target is rejected for now (DATA-frame routing lands when
                // the recv side of H2Transport is wired up).
                Frame::PushPromise { .. } | Frame::Continuation { .. } | Frame::Data { .. } => {
                    return Err(H2ErrorCode::ProtocolError.into());
                }

                // Benign frames whose effect is not yet implemented. Tolerate to keep the
                // handshake clean until the relevant phases.
                Frame::SettingsAck
                | Frame::Ping { ack: true, .. }
                | Frame::WindowUpdate { .. }
                | Frame::RstStream { .. }
                | Frame::Priority { .. }
                | Frame::Unknown { .. } => {}
            }
        }
    }

    /// Open a new request stream from a HEADERS frame the driver has just decoded.
    ///
    /// Validates the stream id (odd + monotonically increasing per §5.1.1), reassembles any
    /// trailing CONTINUATION frames into a single block (§6.10 — no other frame may interleave
    /// on any stream while the block is in progress), HPACK-decodes the assembled bytes, and
    /// returns the [`H2Transport`] that the runtime adapter will spawn a handler task against.
    ///
    /// `header_prefix_consumed` is the offset in `self.read_buf` past the HEADERS frame's fixed
    /// prefix (frame header + optional pad-length byte + optional priority block) — i.e. the
    /// start of the first header block fragment.
    async fn open_stream(
        &mut self,
        stream_id: u32,
        end_stream: bool,
        first_end_headers: bool,
        first_block_length: u32,
        header_prefix_consumed: usize,
    ) -> Result<H2Transport, H2Error> {
        // §5.1.1: a peer-initiated stream id must be odd and strictly greater than every prior
        // peer-initiated stream id.
        if stream_id.is_multiple_of(2)
            || stream_id <= self.last_peer_stream_id
            || self.streams.contains_key(&stream_id)
        {
            return Err(H2ErrorCode::ProtocolError.into());
        }

        // Copy the first fragment out before we reuse `read_buf` for any CONTINUATION frame.
        let block_start = header_prefix_consumed;
        let block_end = block_start
            .checked_add(
                usize::try_from(first_block_length).map_err(|_| H2ErrorCode::FrameSizeError)?,
            )
            .ok_or(H2ErrorCode::FrameSizeError)?;
        if block_end > self.read_buf.len() {
            return Err(H2ErrorCode::FrameSizeError.into());
        }
        let mut block = self.read_buf[block_start..block_end].to_vec();

        // Tight loop reading CONTINUATION frames until END_HEADERS. §6.10 forbids any other
        // frame (on any stream) interleaving — the caller does no swansong interruption here so
        // we can't be cancelled mid-block.
        let mut end_headers = first_end_headers;
        while !end_headers {
            let (frame, consumed) = read_frame(&mut self.transport, &mut self.read_buf).await?;
            let Frame::Continuation {
                stream_id: cont_stream_id,
                end_headers: cont_end_headers,
                header_block_length,
            } = frame
            else {
                return Err(H2ErrorCode::ProtocolError.into());
            };
            if cont_stream_id != stream_id {
                return Err(H2ErrorCode::ProtocolError.into());
            }
            let cont_start = consumed;
            let cont_end = cont_start
                .checked_add(
                    usize::try_from(header_block_length)
                        .map_err(|_| H2ErrorCode::FrameSizeError)?,
                )
                .ok_or(H2ErrorCode::FrameSizeError)?;
            if cont_end > self.read_buf.len() {
                return Err(H2ErrorCode::FrameSizeError.into());
            }
            block.extend_from_slice(&self.read_buf[cont_start..cont_end]);
            end_headers = cont_end_headers;
        }

        let field_section = self.hpack.decode(&block)?;

        let _ = end_stream; // recv-EOF wiring lands in the next commit (DATA routing).

        let state = Arc::new(StreamState::default());
        self.streams.insert(stream_id, state.clone());
        self.last_peer_stream_id = stream_id;

        Ok(H2Transport::new(
            self.connection.clone(),
            stream_id,
            field_section,
            state,
        ))
    }
}

async fn read_preface<T>(transport: &mut T) -> Result<(), H2Error>
where
    T: AsyncRead + Unpin + Send,
{
    let mut preface = [0u8; 24];
    transport.read_exact(&mut preface).await?;
    if &preface != CLIENT_PREFACE {
        return Err(H2ErrorCode::ProtocolError.into());
    }
    Ok(())
}

/// Read one frame from `transport`, reusing `buf` for both the header and the payload.
///
/// Returns the decoded frame plus the byte offset within `buf` past the frame's fixed prefix —
/// for body-bearing frames (DATA / HEADERS / CONTINUATION / `PUSH_PROMISE` / Unknown) the actual
/// payload bytes start at this offset and run for the type-specific length carried inside the
/// `Frame`. For control frames the entire payload has already been consumed into the `Frame`.
///
/// At return, `buf` holds exactly `FRAME_HEADER_LEN + payload_length` bytes.
async fn read_frame<T>(transport: &mut T, buf: &mut Vec<u8>) -> Result<(Frame, usize), H2Error>
where
    T: AsyncRead + Unpin + Send,
{
    buf.resize(FRAME_HEADER_LEN, 0);
    transport.read_exact(&mut buf[..FRAME_HEADER_LEN]).await?;
    let header = FrameHeader::decode(&buf[..FRAME_HEADER_LEN])
        .expect("read_exact filled FRAME_HEADER_LEN bytes");

    let payload_len = usize::try_from(header.length).map_err(|_| H2ErrorCode::FrameSizeError)?;
    if payload_len > MAX_BUFFER_SIZE {
        return Err(H2ErrorCode::FrameSizeError.into());
    }
    buf.resize(FRAME_HEADER_LEN + payload_len, 0);
    if payload_len > 0 {
        transport.read_exact(&mut buf[FRAME_HEADER_LEN..]).await?;
    }

    match Frame::decode(buf) {
        Ok((frame, consumed)) => Ok((frame, consumed)),
        Err(FrameDecodeError::Error(code)) => Err(code.into()),
        // Frame::decode only returns Incomplete if fewer bytes are available than a control frame
        // requires; we read exactly `header.length` payload bytes, so this is unreachable.
        Err(FrameDecodeError::Incomplete) => Err(H2ErrorCode::FrameSizeError.into()),
    }
}

async fn write_settings<T>(transport: &mut T, settings: &H2Settings) -> Result<(), H2Error>
where
    T: AsyncWrite + Unpin + Send,
{
    let mut buf = vec![0u8; frame::settings::encoded_len(settings)];
    let n = frame::settings::encode(settings, &mut buf).expect("buffer sized from encoded_len");
    transport.write_all(&buf[..n]).await?;
    transport.flush().await?;
    Ok(())
}

async fn write_settings_ack<T>(transport: &mut T) -> Result<(), H2Error>
where
    T: AsyncWrite + Unpin + Send,
{
    let mut buf = [0u8; frame::settings::ACK_ENCODED_LEN];
    frame::settings::encode_ack(&mut buf).expect("ACK_ENCODED_LEN is exactly the fixed ack size");
    transport.write_all(&buf).await?;
    transport.flush().await?;
    Ok(())
}

async fn write_ping_ack<T>(transport: &mut T, opaque_data: [u8; 8]) -> Result<(), H2Error>
where
    T: AsyncWrite + Unpin + Send,
{
    let mut buf = [0u8; frame::ping::ENCODED_LEN];
    frame::ping::encode(opaque_data, true, &mut buf).expect("ENCODED_LEN matches fixed ping size");
    transport.write_all(&buf).await?;
    transport.flush().await?;
    Ok(())
}

async fn send_goaway<T>(
    transport: &mut T,
    last_stream_id: u32,
    error_code: H2ErrorCode,
) -> Result<(), H2Error>
where
    T: AsyncWrite + Unpin + Send,
{
    let mut buf = vec![0u8; frame::goaway::encoded_len(0)];
    frame::goaway::encode(last_stream_id, error_code, &[], &mut buf)
        .expect("buffer sized from encoded_len");
    transport.write_all(&buf).await?;
    transport.flush().await?;
    Ok(())
}
