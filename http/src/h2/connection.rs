//! HTTP/2 connection driver (RFC 9113).
//!
//! [`H2Connection`] is the shared, [`Arc`]-able per-connection state — handler tasks reference it
//! by way of their [`H2Transport`] to talk back to the driver. [`H2Acceptor`] owns the underlying
//! TCP transport and the demux state, and is driven by the runtime adapter via
//! [`H2Acceptor::next`]: each call returns the next opened request stream (an [`H2Transport`] for
//! the runtime to spawn a handler task against), or `None` when the connection is closed.
//!
//! Phase 3 (in progress): the acceptor performs the preface exchange, ACKs SETTINGS, echoes PINGs,
//! and shuts the connection down cleanly on local or peer signal. Stream open / DATA / send-side
//! land incrementally on top of this skeleton.
//!
//! [`H2Transport`]: super::transport::H2Transport

use super::{
    H2Error, H2ErrorCode, H2Settings,
    frame::{self, FRAME_HEADER_LEN, Frame, FrameDecodeError, FrameHeader},
};
use crate::HttpContext;
use futures_lite::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use std::sync::Arc;
use swansong::{ShutdownCompletion, Swansong};

/// The client connection preface (RFC 9113 §3.4). 24 bytes the client MUST send before any
/// HTTP/2 frames.
pub(crate) const CLIENT_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// Upper bound for transient frame buffers — prevents runaway allocation on a peer that advertises
/// an absurd `MAX_FRAME_SIZE`. The per-connection maximum is negotiated via SETTINGS and will
/// replace this in phase 7.
const MAX_BUFFER_SIZE: usize = 1 << 20;

/// Shared per-connection state for HTTP/2.
///
/// Wrapped in an [`Arc`] and held by both the [`H2Acceptor`] driver and every [`H2Transport`]
/// handed to a handler task. Per-stream tables, HPACK decoder state, and flow-control bookkeeping
/// will all live here as later phases land.
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
///
/// [`H2Transport`]: super::transport::H2Transport
#[derive(Debug)]
pub struct H2Acceptor<T> {
    connection: Arc<H2Connection>,
    transport: T,
    state: AcceptorState,
    /// Reusable scratch buffer for the next frame's header + payload.
    read_buf: Vec<u8>,
    /// Set to `true` when the driver has emitted a final GOAWAY (graceful or PROTOCOL_ERROR) and
    /// `Self::next` should subsequently return `Ok(None)`.
    finished: bool,
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
    /// Phase 3 is incremental: this method does not yet open streams. It runs the preface +
    /// SETTINGS handshake + PING/GOAWAY housekeeping, returning `Ok(None)` once shut down.
    ///
    /// # Errors
    ///
    /// Returns an [`H2Error`] for any protocol violation detected while decoding peer frames or
    /// for an unrecoverable transport I/O error. A final GOAWAY is sent before this method returns
    /// (best-effort; I/O errors skip it).
    ///
    /// [`H2Transport`]: super::transport::H2Transport
    pub async fn next(&mut self) -> Result<Option<TransportPlaceholder>, H2Error> {
        if self.finished {
            return Ok(None);
        }

        let result = self.drive().await;
        self.finished = true;

        // Translate driver outcomes to next()'s contract. A clean exit means the connection is
        // done — emit a graceful GOAWAY and return None on subsequent calls. A protocol error
        // means GOAWAY with the offending code, then the error propagates. I/O errors skip GOAWAY
        // since the transport is already unusable.
        match result {
            Ok(()) => {
                let _ = send_goaway(&mut self.transport, 0, H2ErrorCode::NoError).await;
                Ok(None)
            }
            Err(H2Error::Protocol(code)) => {
                let _ = send_goaway(&mut self.transport, 0, code).await;
                Err(H2Error::Protocol(code))
            }
            Err(e @ H2Error::Io(_)) => Err(e),
        }
    }

    /// Inner loop body. Returns `Ok(())` on a clean shutdown (peer GOAWAY or local swansong),
    /// `Err` on any protocol or I/O failure.
    async fn drive(&mut self) -> Result<(), H2Error> {
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

            let Some(frame) = read else {
                return Ok(());
            };

            match frame? {
                Frame::Settings(_) => write_settings_ack(&mut self.transport).await?,

                Frame::Ping {
                    opaque_data,
                    ack: false,
                } => write_ping_ack(&mut self.transport, opaque_data).await?,

                Frame::Goaway { .. } => {
                    self.connection.swansong.shut_down();
                    return Ok(());
                }

                // PUSH_PROMISE from a client is always a connection error (§6.6). Until stream
                // machinery lands, DATA / HEADERS / CONTINUATION are equally invalid here.
                Frame::PushPromise { .. }
                | Frame::Data { .. }
                | Frame::Headers { .. }
                | Frame::Continuation { .. } => {
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
}

/// Placeholder return type for [`H2Acceptor::next`] until [`H2Transport`] lands in a follow-up
/// commit. Today no value of this type is ever constructed — `next` only ever returns
/// `Ok(None)` or an error — but having the slot in place keeps the API shape stable as stream
/// machinery is added.
///
/// [`H2Transport`]: super::transport::H2Transport
#[derive(Debug, Clone, Copy)]
pub enum TransportPlaceholder {}

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

/// Read one frame from `transport`, reusing `buf` for both the header and the payload. The buffer
/// ends the call holding exactly `FRAME_HEADER_LEN + payload_length` bytes.
async fn read_frame<T>(transport: &mut T, buf: &mut Vec<u8>) -> Result<Frame, H2Error>
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
        Ok((frame, _)) => Ok(frame),
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
