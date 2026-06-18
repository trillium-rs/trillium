//! Component types used in the definition of `h2::acceptor`

use crate::{
    Conn, HttpConfig, Priority,
    h2::{
        H2Driver, H2Error, H2ErrorCode, H2Transport,
        acceptor::{inflow::Inflow, send::SendCursor},
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
    max_header_list_size: u64,
    copy_loops_per_yield: usize,
    hpack_table_capacity: usize,
}

impl AcceptorConfig {
    pub(super) fn from_http_config(config: &HttpConfig) -> Self {
        let initial_stream_window_size = config.h2_initial_stream_window_size();
        let configured_max = config.h2_max_stream_recv_window_size();
        // The post-read window target must be at least the advertised initial: a smaller `max`
        // would leave the stream stuck above its own "max" (promotion via `Inflow::raise_target`
        // can only grow the window, never shrink it). Coerce it up and warn once so the
        // misconfiguration is visible without spamming the log on every connection.
        let max_stream_recv_window_size = configured_max.max(initial_stream_window_size);
        if max_stream_recv_window_size != configured_max {
            warn_misordered_stream_window_config(initial_stream_window_size, configured_max);
        }
        Self {
            initial_stream_window_size,
            max_stream_recv_window_size,
            initial_connection_window_size: config.h2_initial_connection_window_size(),
            max_concurrent_streams: config.h2_max_concurrent_streams(),
            max_frame_size: config.h2_max_frame_size(),
            max_header_list_size: config.max_header_list_size(),
            copy_loops_per_yield: config.copy_loops_per_yield(),
            hpack_table_capacity: config.dynamic_table_capacity(),
        }
    }
}

/// Warn (at most once per process) that `h2_max_stream_recv_window_size` was configured below
/// `h2_initial_stream_window_size` and has been clamped up to it. Best-effort once — a second
/// differently-misconfigured `HttpConfig` won't re-warn — which is fine for a setup-time hint.
fn warn_misordered_stream_window_config(initial: u32, configured_max: u32) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        log::warn!(
            "h2_max_stream_recv_window_size ({configured_max}) is below \
             h2_initial_stream_window_size ({initial}); clamping the per-stream recv window up to \
             the initial. Set max >= initial to silence this."
        );
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
    /// reset to the client — the sequence the h2 spec and most clients assume. Any
    /// inbound bytes the peer happens to send during this window are discarded; we've
    /// already committed to closing.
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

    /// Per-stream send flow-control window. Seeded from
    /// `peer_settings.effective_initial_window_size()` when the stream is opened;
    /// decremented as we emit DATA frames; incremented by peer
    /// `WINDOW_UPDATE(stream_id, inc)`; adjusted by `SETTINGS_INITIAL_WINDOW_SIZE` delta on
    /// mid-connection SETTINGS change (may drive negative). Overflow past
    /// [`MAX_FLOW_CONTROL_WINDOW`] is a stream-level `FLOW_CONTROL_ERROR` (→ `RST_STREAM`).
    pub(super) send_window: i64,

    /// Per-stream receive flow-control window (RFC 9113 §6.9). Seeded at the advertised
    /// `SETTINGS_INITIAL_WINDOW_SIZE`; promoted to `h2_max_stream_recv_window_size` once the
    /// handler signals it intends to read the body (`is_reading`), then topped up as the handler
    /// drains. See [`Inflow`] for the accounting model. A peer that sends past the granted window
    /// earns a connection-level `FLOW_CONTROL_ERROR`.
    pub(super) stream_inflow: Inflow,

    /// Declared request-body length from the `content-length` request header, if present and
    /// parseable. The driver tallies inbound DATA octets against this to enforce RFC 9113
    /// §8.1.2.6: a body whose length disagrees with `content-length` is a stream-level
    /// `PROTOCOL_ERROR`. `None` means no declared length, so no check applies.
    pub(super) expected_content_length: Option<u64>,

    /// Running total of inbound DATA payload octets (body bytes only — pad-length byte and
    /// padding excluded) received on this stream, compared against `expected_content_length`.
    pub(super) received_body_len: u64,

    /// RFC 9218 priority parsed from the request's `priority` header at stream open (or the
    /// default when absent). The send pump's fallback when no `PRIORITY_UPDATE` has overridden
    /// it — see [`H2Driver::effective_priority`][super::H2Driver::effective_priority]. Always
    /// the default on client-role streams, which carry no scheduling signal of their own.
    pub(super) priority: Priority,
}

impl StreamEntry {
    pub(super) fn new(
        shared: Arc<StreamState>,
        send_window: i64,
        stream_inflow: Inflow,
        expected_content_length: Option<u64>,
        priority: Priority,
    ) -> Self {
        Self {
            shared,
            send: None,
            send_window,
            stream_inflow,
            expected_content_length,
            received_body_len: 0,
            priority,
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
