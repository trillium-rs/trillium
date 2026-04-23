//! HTTP/2 connection driver (RFC 9113).
//!
//! Phase 1 responsibility: own the transport, perform the preface exchange, ACK peer SETTINGS,
//! echo PINGs, and shut the connection down cleanly on local or peer signal. Stream machinery
//! (DATA, HEADERS, flow control) lands in phase 3.
//!
//! The loop is straight-line `async`: one task reads a frame header, reads its payload, dispatches,
//! and optionally writes a response. The more elaborate coordinator shape described in the planning
//! doc (`AtomicWaker` + per-stream send buffers) appears when streams do.

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

/// Shared state for a single HTTP/2 TCP connection.
///
/// Phase 1 state is minimal: a handle to the shared [`HttpContext`] and a connection-scoped
/// [`Swansong`] for shutdown propagation. Per-stream state, HPACK tables, and flow-control
/// bookkeeping land in later phases.
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

    /// Drive the connection to completion.
    ///
    /// Reads the client preface, exchanges SETTINGS, then enters the frame dispatch loop until
    /// either the peer sends GOAWAY, the connection-scoped swansong shuts down, or a protocol
    /// violation occurs. A terminating GOAWAY is sent on any clean exit; I/O errors skip it.
    ///
    /// # Errors
    ///
    /// Returns an [`H2Error`] for any protocol violation detected while decoding peer frames or
    /// for an unrecoverable transport I/O error.
    pub async fn run<T>(self: Arc<Self>, mut transport: T) -> Result<(), H2Error>
    where
        T: AsyncRead + AsyncWrite + Unpin + Send,
    {
        let _guard = self.swansong.guard();
        let result = self.drive(&mut transport).await;

        // Always try a final GOAWAY before the transport drops. On I/O failure the transport is
        // already unusable, so skip it.
        match &result {
            Ok(()) => {
                let _ = send_goaway(&mut transport, 0, H2ErrorCode::NoError).await;
            }
            Err(H2Error::Protocol(code)) => {
                let _ = send_goaway(&mut transport, 0, *code).await;
            }
            Err(H2Error::Io(_)) => {}
        }
        result
    }

    async fn drive<T>(&self, transport: &mut T) -> Result<(), H2Error>
    where
        T: AsyncRead + AsyncWrite + Unpin + Send,
    {
        read_preface(transport).await?;

        // Server-side preface per §3.4: our initial SETTINGS frame MUST be the first frame we send.
        write_settings(transport, &H2Settings::server_defaults()).await?;

        let mut buf = vec![0u8; FRAME_HEADER_LEN];

        loop {
            match self
                .swansong
                .interrupt(read_frame(transport, &mut buf))
                .await
            {
                None => return Ok(()),
                Some(result) => match result? {
                    Frame::Settings(_) => write_settings_ack(transport).await?,

                    Frame::Ping {
                        opaque_data,
                        ack: false,
                    } => write_ping_ack(transport, opaque_data).await?,

                    Frame::Goaway { .. } => {
                        self.swansong.shut_down();
                        return Ok(());
                    }

                    // PUSH_PROMISE from a client is always a connection error (§6.6). Phase 1
                    // has no streams so DATA/HEADERS/CONTINUATION are equally invalid; stream
                    // machinery in phase 3 will dispatch them instead.
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
                },
            }
        }
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
