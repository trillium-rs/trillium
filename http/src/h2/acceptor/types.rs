//! Component types used in the definition of `h2::acceptor`

use crate::{
    Conn, HttpConfig,
    h2::{
        H2Driver, H2Error, H2ErrorCode, H2Transport, acceptor::send::SendCursor,
        transport::StreamState,
    },
};
use futures_lite::{AsyncRead, AsyncWrite, stream::Stream};
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

/// h2-relevant configuration extracted from [`HttpConfig`][crate::HttpConfig] at acceptor
/// construction. Carried as a plain value so hot-loop policy checks don't cross the
/// `Arc<HttpContext>` indirection.
#[derive(Debug, Clone, Copy, fieldwork::Fieldwork)]
#[fieldwork(get)]
pub(in crate::h2) struct AcceptorConfig {
    initial_stream_window_size: u32,
    max_stream_recv_window_size: u32,
    initial_connection_window_size: u32,
    max_concurrent_streams: u32,
    max_frame_size: u32,
    copy_loops_per_yield: usize,
    hpack_table_capacity: usize,
}

impl AcceptorConfig {
    pub(super) fn from_http_config(config: &HttpConfig) -> Self {
        Self {
            initial_stream_window_size: config.h2_initial_stream_window_size(),
            max_stream_recv_window_size: config.h2_max_stream_recv_window_size(),
            initial_connection_window_size: config.h2_initial_connection_window_size(),
            max_concurrent_streams: config.h2_max_concurrent_streams(),
            max_frame_size: config.h2_max_frame_size(),
            copy_loops_per_yield: config.copy_loops_per_yield(),
            hpack_table_capacity: config.dynamic_table_capacity(),
        }
    }
}

/// Position of the connection in its high-level lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DriverState {
    /// Haven't read the client preface yet.
    AwaitingPreface,
    /// Preface read; need to queue our initial SETTINGS frame to `write_buf`.
    NeedsServerSettings,
    /// Steady state — read frames from the transport and dispatch.
    Running,
    /// GOAWAY has been queued; drain `write_buf` then transition to [`Drained`] (for
    /// graceful shutdown) or terminate directly (for I/O error paths where the transport
    /// is already untrustworthy).
    ///
    /// [`Drained`]: Self::Drained
    Closing,
    /// Our outbound bytes are on the wire (including our GOAWAY). Now we're waiting for
    /// the peer to close its write half (recv returns 0) so our Drop doesn't look like a
    /// reset to the client — the sequence the h2 spec and most clients (hyper-h2 in
    /// particular) assume. Any inbound bytes the peer happens to send during this window
    /// are discarded; we've already committed to closing.
    Drained,
}

/// Where the read cursor is inside the current frame.
#[derive(Debug, Clone, Copy)]
pub(super) enum ReadPhase {
    /// Not yet read the 9 bytes of the next frame header.
    NeedHeader,
    /// Header read and validated; still collecting payload bytes. `total` is the full target
    /// fill (`FRAME_HEADER_LEN + payload_len`). The decoded header itself is cheap enough to
    /// re-parse from the buffer when we dispatch, so we don't stash it here.
    NeedPayload { total: usize },
}

/// Why the driver is closing — shaped around what the terminal `drive` result should be.
#[derive(Debug)]
pub(super) enum CloseOutcome {
    /// Clean close. `drive` returns `None`.
    Graceful,
    /// Protocol error. `drive` returns `Some(Err(...))`. GOAWAY with this code has been
    /// queued.
    Protocol(H2ErrorCode),
    /// I/O error. GOAWAY was NOT queued (transport is untrustworthy). Propagated verbatim.
    Io(io::Error),
}

/// Driver-side view of a single open stream: the shared state the handler also sees, plus a
/// cache of decisions the driver has made for this stream (which the handler doesn't need
/// to know).
#[derive(Debug)]
pub(super) struct StreamEntry {
    /// Shared state (recv buffer, send slot, handler wakers). Owned by `Arc` so the
    /// handler task can outlive or operate concurrently with the driver's view.
    pub(super) shared: Arc<StreamState>,

    /// Driver-private send-side state for an in-progress response. `None` until the conn
    /// task submits a response via [`H2Connection::submit_send`] and the driver picks it
    /// up on its next `service_handler_signals` tick.
    ///
    /// [`H2Connection::submit_send`]: super::H2Connection::submit_send
    pub(super) send: Option<SendCursor>,

    /// Per-stream send flow-control window (RFC 9113 §6.9). Seeded from
    /// `peer_settings.effective_initial_window_size()` when the stream is opened;
    /// decremented as we emit DATA frames; incremented by peer
    /// `WINDOW_UPDATE(stream_id, inc)`; adjusted by `SETTINGS_INITIAL_WINDOW_SIZE` delta on
    /// mid-connection SETTINGS change (§6.9.2 — may drive negative). Overflow past
    /// [`MAX_FLOW_CONTROL_WINDOW`] is a stream-level `FLOW_CONTROL_ERROR` (→ `RST_STREAM`).
    pub(super) send_window: i64,

    /// Per-stream recv flow-control window (RFC 9113 §6.9) — how many bytes we've told
    /// the peer it may still send on this stream. Starts at the server's advertised
    /// `SETTINGS_INITIAL_WINDOW_SIZE` (currently 0 — lazy-WU pattern); decremented as the
    /// peer's DATA frames arrive; incremented as we emit stream-level `WINDOW_UPDATE`
    /// (both the initial raise on the handler's `is_reading` signal and every subsequent
    /// refill crediting bytes the handler has consumed). A negative value means the peer
    /// overran the window — connection-level `FLOW_CONTROL_ERROR`.
    pub(super) peer_recv_window: i64,
}

impl StreamEntry {
    pub(super) fn new(shared: Arc<StreamState>, send_window: i64, peer_recv_window: i64) -> Self {
        Self {
            shared,
            send: None,
            send_window,
            peer_recv_window,
        }
    }
}

/// Result of dispatching one decoded frame.
pub(super) enum Action {
    /// Frame handled; continue the main loop.
    Continue,
    /// A stream just opened and the request validated — return the [`Conn`] to the caller;
    /// the runtime adapter spawns a handler task per emitted Conn. Boxed to keep the enum
    /// small — `Conn` is over 500 bytes and most dispatches return `Continue`.
    Emit(Box<Conn<H2Transport>>),
    /// Begin graceful or erroring close with this outcome.
    Close(CloseOutcome),
}

/// Future returned by [`H2Driver::next`]. Resolves to `None` on graceful close, `Some(Ok)`
/// when a new request stream opens, or `Some(Err)` on a fatal protocol or I/O error.
#[must_use = "futures do nothing unless awaited"]
#[derive(Debug)]
pub struct Next<'a, T> {
    pub(super) driver: &'a mut H2Driver<T>,
}

impl<T> Future for Next<'_, T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    type Output = Option<Result<Conn<H2Transport>, H2Error>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.driver.drive(cx)
    }
}

impl<T> Stream for H2Driver<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    type Item = Result<Conn<H2Transport>, H2Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().drive(cx)
    }
}

/// Slice the interesting bytes out of a just-read frame. Bounds-checks to defend against a
/// payload length on the wire that disagrees with a body-bearing frame's declared inner
/// length.
pub(super) fn frame_slice(
    buf: &[u8],
    start: usize,
    length: u32,
    total: usize,
) -> Result<&[u8], CloseOutcome> {
    let length =
        usize::try_from(length).map_err(|_| CloseOutcome::Protocol(H2ErrorCode::FrameSizeError))?;
    let end = start
        .checked_add(length)
        .ok_or(CloseOutcome::Protocol(H2ErrorCode::FrameSizeError))?;
    if end > total {
        return Err(CloseOutcome::Protocol(H2ErrorCode::FrameSizeError));
    }
    Ok(&buf[start..end])
}
