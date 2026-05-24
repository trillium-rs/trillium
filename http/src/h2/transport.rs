//! Per-stream transport handed to handler tasks.
//!
//! [`H2Transport`] is the [`AsyncRead`] + [`AsyncWrite`] view of a single HTTP/2 stream. It is
//! carried on the emitted [`Conn`][crate::Conn] returned from [`H2Driver::next`], and the
//! runtime adapter spawns a handler task that consumes it. The transport never touches the
//! underlying TCP connection directly — all I/O coordinates through shared per-stream state
//! on the [`H2Connection`] driven by the driver task.
//!
//! Two paths reach the impls:
//!
//! - **Normal HTTP/2 request/response**: handlers usually don't touch [`H2Transport`] directly
//!   (same sharp edge h1 and h3 document). [`ReceivedBody`][crate::ReceivedBody] reads request body
//!   bytes through the transport's `AsyncRead`. Response bytes are handed to the driver as a queue
//!   of [`OutboundPart`]s via [`H2Connection::submit_send`][submit_send]; the send pump frames them
//!   without ever touching this `AsyncWrite`.
//!
//! - **Extended-CONNECT upgrades** ([RFC 8441] WebSocket-over-h2, plus the in-progress
//!   `draft-ietf-webtrans-http2` for WebTransport-over-h2): after the handler responds 200 to a
//!   `CONNECT` request with a `:protocol` pseudo-header, [`Conn::send_h2`][crate::Conn::send_h2]
//!   routes through [`H2Connection::submit_upgrade`][submit_upgrade], which enqueues HEADERS (and
//!   an optional prelude body) *without* a terminating [`OutboundPart::Close`], leaving the stream
//!   open as a bidirectional byte channel. The runtime adapter then dispatches
//!   [`Handler::upgrade`][trillium::Handler::upgrade], which gets an [`Upgrade`][crate::Upgrade]
//!   wrapping this transport. `AsyncWrite::poll_write` appends to a per-stream outbound ring
//!   ([`SendState::outbound`]); the send pump drains it into DATA frames bounded by the per-stream
//!   and connection send windows. `AsyncWrite::poll_close` enqueues [`OutboundPart::Close`] so the
//!   driver emits the `END_STREAM` terminator and tears the stream down.
//!
//! [`H2Driver::next`]: super::H2Driver::next
//! [`H2Connection`]: super::H2Connection
//! [submit_send]: super::H2Connection::submit_send
//! [submit_upgrade]: super::H2Connection::submit_upgrade
//! [RFC 8441]: https://www.rfc-editor.org/rfc/rfc8441

use super::{
    H2Connection, H2ErrorCode,
    stream_state::{StreamEvent, StreamFsm},
};
use crate::{
    Body, Buffer, Headers,
    headers::hpack::{FieldSection, PseudoHeaders},
};
use atomic_waker::AtomicWaker;
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    collections::VecDeque,
    fmt, io,
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll},
};

/// A single HTTP/2 stream's transport handle.
///
/// Carries a backref to the shared [`H2Connection`], the stream id, and the per-stream
/// `Arc<StreamState>` used by the read and write sides. Normal HTTP/2 operation reads through
/// [`ReceivedBody`][crate::ReceivedBody] and writes through the driver's send queue; the
/// `AsyncRead` / `AsyncWrite` impls here are only reached by code that borrows the transport
/// directly (typically an upgrade handler after extended CONNECT).
pub struct H2Transport {
    connection: Arc<H2Connection>,
    stream_id: u32,
    state: Arc<StreamState>,
}

impl fmt::Debug for H2Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("H2Transport")
            .field("stream_id", &self.stream_id)
            .finish_non_exhaustive()
    }
}

impl H2Transport {
    /// Create a transport for a stream that has just been opened by the driver.
    pub(super) fn new(
        connection: Arc<H2Connection>,
        stream_id: u32,
        state: Arc<StreamState>,
    ) -> Self {
        Self {
            connection,
            stream_id,
            state,
        }
    }

    /// The stream identifier this transport is bound to.
    pub fn stream_id(&self) -> u32 {
        self.stream_id
    }

    /// The shared [`H2Connection`] backing this stream.
    pub fn connection(&self) -> &Arc<H2Connection> {
        &self.connection
    }
}

impl Drop for H2Transport {
    /// Application-side release / cancel signal, decided from the stream's protocol state plus
    /// whether a response has been handed off:
    ///
    /// - **Already removed from the shared map** — the driver beat us to cleanup; no-op.
    /// - **`Closed`** (both halves done on the wire; client-role streams linger in the map for
    ///   post-EOF trailer/response access) — set [`SendState::transport_dropped`] so the driver GCs
    ///   the lingering entry.
    /// - **Send half still open + a response was submitted** (`submit_resolved`) — a bidirectional
    ///   upgrade tunnel the handler is dropping; enqueue [`OutboundPart::Close`] for a graceful
    ///   `END_STREAM`.
    /// - **Anything else** (`HalfClosedLocal` awaiting the peer, or send-open with no response ever
    ///   submitted) — abandoning the stream; enqueue `RST_STREAM(Cancel)`.
    fn drop(&mut self) {
        if !self.connection.streams_lock().contains_key(&self.stream_id) {
            log::trace!(
                "h2 stream {}: H2Transport dropped on already-released stream",
                self.stream_id,
            );
            return;
        }

        let fsm = *self.state.fsm_lock();
        if fsm.is_closed() {
            log::trace!(
                "h2 stream {}: H2Transport dropped on wire-closed stream — releasing",
                self.stream_id,
            );
            self.state
                .send
                .transport_dropped
                .store(true, Ordering::Release);
        } else if !fsm.send_closed() && self.state.send.submit_resolved.load(Ordering::Acquire) {
            // Send half open and a response is on the wire: an upgrade tunnel being dropped.
            // Graceful close.
            log::trace!(
                "h2 stream {}: H2Transport dropped (upgrade tunnel) — scheduling graceful close",
                self.stream_id,
            );
            self.state.request_close();
        } else {
            // Send half closed awaiting the peer, or never-responded: abandon the stream.
            log::debug!(
                "h2 stream {}: H2Transport dropped mid-stream — RST_STREAM(Cancel)",
                self.stream_id,
            );
            self.state.request_reset(H2ErrorCode::Cancel);
        }
        self.state.needs_servicing.store(true, Ordering::Release);
        self.state.send.outbound_write_waker.wake();
        self.connection.outbound_waker().wake();
    }
}

impl AsyncRead for H2Transport {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if out.is_empty() {
            return Poll::Ready(Ok(0));
        }

        // The first `poll_read` is the handler's declaration of intent to consume the request
        // body — until this point, we've advertised a zero recv window and the peer has sent
        // nothing beyond HEADERS. Tell the driver to top up our per-stream window now. Later
        // calls CAS-fail silently and don't re-signal.
        let recv_state = &self.state.recv;
        let connection = &*self.connection;
        if !recv_state.is_reading.swap(true, Ordering::AcqRel) {
            self.state.needs_servicing.store(true, Ordering::Release);
            connection.outbound_waker().wake();
        }

        let mut recv = recv_state.buf.lock().expect("recv buf mutex poisoned");

        // Copy as many bytes as fit from the front of the ring into `out`, then advance the
        // ring's virtual read cursor. `Buffer::ignore_front` truncates the underlying `Vec` to
        // zero when we drain fully, so capacity stays bounded by peak in-flight bytes rather
        // than cumulative traffic.
        let take = out.len().min(recv.len());
        if take > 0 {
            out[..take].copy_from_slice(&recv[..take]);
            recv.ignore_front(take);
            drop(recv);
            recv_state
                .bytes_consumed
                .fetch_add(take as u64, Ordering::AcqRel);
            self.state.needs_servicing.store(true, Ordering::Release);
            connection.outbound_waker().wake();
            return Poll::Ready(Ok(take));
        }

        // Buffer empty. Register the waker *before* releasing the buf lock so a driver push
        // between this poll and the recv-closed check is guaranteed to wake us:
        //   1. We take buf lock (driver-push blocked).
        //   2. We register waker.
        //   3. We drop buf lock (driver-push may now proceed and fire waker).
        //   4. We read recv-closed from the FSM.
        //   5. Return Pending or Ready(0); if a push raced through step 3, the waker is registered
        //      and a fresh poll will see the new bytes.
        recv_state.waker.register(cx.waker());
        drop(recv);
        if self.state.fsm_lock().recv_closed() {
            return Poll::Ready(Ok(0));
        }
        Poll::Pending
    }
}

impl AsyncWrite for H2Transport {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        // Parity with h1/h3: writing to a stream whose send half is already closed (clean
        // `END_STREAM`, or reset) is a `BrokenPipe` rather than silently swallowed bytes.
        // Otherwise we always accept the write — the "don't write at an inappropriate time"
        // contract is the caller's, same as borrowing the raw transport in h1/h3.
        if self.state.fsm_lock().send_closed() {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }

        // Append into the per-stream outbound ring. The send pump drains the same ring into DATA
        // frames bounded by per-stream + connection send windows. Bounded by
        // `response_buffer_max_len` (the cap h1 and h3 use for their transit buffers): a stalled
        // peer window means the driver can't drain, the cap is hit, and we return `Pending` so the
        // handler is throttled. The drain side wakes `outbound_write_waker` after each take.
        let send = &self.state.send;
        let cap = self.connection.context.config.response_buffer_max_len;
        let mut outbound = send.outbound.lock().expect("outbound buf mutex poisoned");
        if outbound.len() >= cap {
            // Register first, then re-check under lock to close the race against the drain side
            // (which takes the same lock to `ignore_front` and then wakes us).
            send.outbound_write_waker.register(cx.waker());
            if outbound.len() >= cap {
                return Poll::Pending;
            }
        }
        let take = (cap - outbound.len()).min(buf.len());
        log::trace!(
            "h2 stream {}: H2Transport::poll_write appending {take}/{} bytes to outbound ring",
            self.stream_id,
            buf.len(),
        );
        outbound.extend_from_slice(&buf[..take]);
        drop(outbound);

        // Wake the driver (parked on the connection-level waker).
        self.connection.outbound_waker().wake();
        Poll::Ready(Ok(take))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Best-effort: bytes appended via `poll_write` are already visible to the driver and will
        // be framed on the next tick. There's no application-level "flushed" state below us.
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Mark the write half closed by enqueuing the `END_STREAM` terminator. Once the driver
        // drains the remaining outbound ring bytes it frames the terminator and tears the stream
        // down. Idempotent — `request_close` is a no-op if a terminator is already queued or the
        // send half is already closed.
        log::trace!(
            "h2 stream {}: H2Transport::poll_close enqueuing Close",
            self.stream_id,
        );
        self.state.request_close();
        self.state.needs_servicing.store(true, Ordering::Release);
        self.state.send.outbound_write_waker.wake();
        self.connection.outbound_waker().wake();
        Poll::Ready(Ok(()))
    }
}

/// A unit of outbound work the conn task hands the driver, framed in order by the send pump.
///
/// `Headers` opens the response/request; `Body` is an owned source the driver frames lazily under
/// flow control (no intermediate buffering); `Trailers` and `Close` are alternative `END_STREAM`
/// terminators; `Reset` abandons the stream (the conn task clears the rest of the queue when it
/// pushes one — nothing else is valid to send after `RST_STREAM`).
///
/// Streaming bytes a handler writes through [`H2Transport`]'s `AsyncWrite` do *not* go here — they
/// flow through the reused [`SendState::outbound`] ring, which the pump drains before framing a
/// terminator.
#[derive(Debug)]
pub(super) enum OutboundPart {
    /// Initial HEADERS block — HPACK-encoded by the driver at frame time so the wire order
    /// matches the dynamic-table mutation order.
    Headers {
        pseudos: PseudoHeaders<'static>,
        headers: Headers,
    },
    /// An owned body source, framed directly into DATA frames under flow control.
    Body(Body),
    /// Trailing HEADERS — carries `END_STREAM`.
    Trailers(Headers),
    /// Empty `DATA(END_STREAM)` terminator.
    Close,
    /// `RST_STREAM(code)` — conn-task-initiated reset; clears any preceding queued parts.
    Reset(H2ErrorCode),
}

impl OutboundPart {
    /// `true` for the `END_STREAM`/`RST_STREAM` terminators — used by `request_close` to stay
    /// idempotent and by the send pump to know the ring must be drained first.
    pub(super) fn is_terminal(&self) -> bool {
        matches!(self, Self::Trailers(_) | Self::Close | Self::Reset(_))
    }
}

/// Shared per-stream state. Owned by an [`Arc`] held jointly by the driver (via the connection's
/// stream table) and the handler task (via [`H2Transport`]).
///
/// The cross-task-visible protocol state machine is the [`StreamFsm`] in [`Self::fsm`] — written
/// only by the driver (which feeds it [`StreamEvent`]s as it frames/receives) and read by both
/// sides. Everything else is data and mailbox plumbing: the recv ring, the outbound parts queue +
/// streaming ring, and the conn-task → driver signals.
#[derive(Debug, Default)]
pub(super) struct StreamState {
    /// Protocol state (RFC 9113 §5.1). Driver is the sole writer; the handler side reads it for
    /// recv-EOF (`poll_read`) and send-closed (`poll_write`) decisions. Held under a `Mutex` so
    /// observe-then-act sequences are atomic.
    fsm: Mutex<StreamFsm>,

    /// Recv side: inbound DATA payloads, handler waker, handler-intent signal, trailers.
    pub(super) recv: RecvState,

    /// Send side: the outbound parts queue, the streaming ring, the driver→conn completion
    /// channel, and the application-release signal.
    pub(super) send: SendState,

    /// Mailbox flag for conn-task → driver work signaling. Set by conn-task code whenever it
    /// produces work the driver should service (enqueue a part, raise `is_reading`, increment
    /// `bytes_consumed`, set `transport_dropped`). The driver's `service_handler_signals` consults
    /// it via `swap(false, AcqRel)` — only streams where it returns `true` pay for the queue-lock
    /// pickup; idle streams cost a single atomic RMW per tick.
    ///
    /// **Setter ordering rule**: write the underlying state first, then store `true` with
    /// `Release`, then wake the connection's outbound waker. Over-notification is harmless;
    /// under-notification would lose a signal.
    pub(super) needs_servicing: AtomicBool,
}

impl StreamState {
    /// Lock the per-stream protocol FSM. Convenience wrapper — every site treats poisoning as a
    /// programming error.
    pub(super) fn fsm_lock(&self) -> MutexGuard<'_, StreamFsm> {
        self.fsm.lock().expect("fsm mutex poisoned")
    }

    /// Apply a [`StreamEvent`] to the FSM. Driver-only — the sole writer of protocol state.
    pub(super) fn fsm_event(
        &self,
        event: StreamEvent,
    ) -> Result<(), super::stream_state::StreamProtocolError> {
        self.fsm_lock().on_event(event)
    }

    /// Stage a full submission of outbound parts atomically, so the send pump sees a complete
    /// message rather than a partial one (the `SubmitSend` future keys "done" off the queue
    /// draining to empty). Raises `needs_servicing`; the caller wakes the driver.
    pub(super) fn stage(&self, parts: impl IntoIterator<Item = OutboundPart>) {
        self.send
            .queue
            .lock()
            .expect("send queue mutex poisoned")
            .extend(parts);
        self.needs_servicing.store(true, Ordering::Release);
    }

    /// Enqueue the `END_STREAM` terminator unless one (or a reset) is already queued or the send
    /// half is already closed. Idempotent — safe to call from repeated `poll_close` / Drop.
    pub(super) fn request_close(&self) {
        if self.fsm_lock().send_closed() {
            return;
        }
        let mut queue = self.send.queue.lock().expect("send queue mutex poisoned");
        if queue.back().is_none_or(|p| !p.is_terminal()) {
            queue.push_back(OutboundPart::Close);
        }
        drop(queue);
        self.needs_servicing.store(true, Ordering::Release);
    }

    /// Clear any queued parts and request `RST_STREAM(code)`. First-wins: a reset already at the
    /// back keeps its code. Nothing else is valid to send after a reset, so clearing the queue
    /// models the §5.1 sequence faithfully. Raises `needs_servicing`; the caller wakes the driver.
    pub(super) fn request_reset(&self, code: H2ErrorCode) {
        let mut queue = self.send.queue.lock().expect("send queue mutex poisoned");
        if matches!(queue.back(), Some(OutboundPart::Reset(_))) {
            return;
        }
        queue.clear();
        queue.push_back(OutboundPart::Reset(code));
        drop(queue);
        self.needs_servicing.store(true, Ordering::Release);
    }
}

/// Receive-side per-stream state.
#[derive(Debug, Default)]
pub(super) struct RecvState {
    /// Inbound DATA body bytes awaiting handler read. A single persistent ring (append-at-tail,
    /// `ignore_front`-at-head): the driver appends via `extend_from_slice` when a DATA frame
    /// arrives; the handler reads from the front and virtually drops consumed bytes. When
    /// `ignore_front` catches up to the data end the `Buffer` truncates to zero, so the underlying
    /// `Vec` capacity stays bounded by peak in-flight bytes rather than cumulative traffic — zero
    /// amortized allocations per DATA frame.
    pub(super) buf: Mutex<Buffer>,

    /// Handler-task waker, fired by the driver after pushing DATA into `buf` or after the FSM
    /// transitions to recv-closed. Single-waiter: only one task ever polls a given `H2Transport`.
    pub(super) waker: AtomicWaker,

    /// Set by the handler's first [`H2Transport::poll_read`] to declare intent to consume the
    /// request body. The driver observes the transition and emits a `WINDOW_UPDATE` for this
    /// stream, topping its recv window up from `SETTINGS_INITIAL_WINDOW_SIZE` (advertised as `0`)
    /// to the per-stream maximum. Once set, stays set.
    pub(super) is_reading: AtomicBool,

    /// Bytes the handler has consumed from `buf` since the driver last sampled this counter.
    /// Incremented by [`H2Transport::poll_read`] using `fetch_add` after each drain; the driver
    /// reads it via `swap(0)` per tick and emits stream-level + connection-level `WINDOW_UPDATE`
    /// for the consumed total.
    pub(super) bytes_consumed: AtomicU64,

    /// Trailers, populated by the driver if a trailing HEADERS frame arrives for this stream.
    /// Always written *before* the FSM transitions to recv-closed, so once the handler observes
    /// `Ready(0)` on the recv side, any trailers for this request are guaranteed to be in place.
    pub(super) trailers: Mutex<Option<Headers>>,

    /// Client-role: response HEADERS field section, populated by the driver on the first non-1xx
    /// HEADERS frame arrival for a client-initiated stream. Server role doesn't use this slot.
    /// Single-shot: the conn task takes the `FieldSection` once; subsequent HEADERS arrivals are
    /// interpreted as trailers. Interim 1xx HEADERS frames are discarded by the driver without
    /// touching this slot or latching `first_response_headers_seen`.
    pub(super) response_headers: Mutex<Option<FieldSection<'static>>>,

    /// Client-role: latching flag for "first HEADERS arrived for this stream." Distinct from
    /// `response_headers.is_some()` — the conn task drains that slot when it consumes headers, so
    /// the driver can't use slot occupancy to distinguish "haven't seen HEADERS yet" from "headers
    /// seen + already taken." Set inside `finalize_response_headers` before that slot is
    /// populated; checked by `route_headers` on subsequent HEADERS to route them as trailers.
    /// Never cleared.
    pub(super) first_response_headers_seen: AtomicBool,

    /// Client-role: waker the conn task registers via
    /// [`H2Connection::response_headers`][super::H2Connection]; fired by the driver after stashing
    /// the `FieldSection` *or* on stream removal (so a parked conn task observing the stream gone
    /// surfaces `NotConnected` instead of hanging).
    pub(super) response_headers_waker: AtomicWaker,
}

/// Send-side per-stream state: the conn-task → driver outbound parts queue, the streaming ring for
/// bidirectional upgrades, the driver → conn-task completion channel, and the application-release
/// signal.
///
/// **Normal response path**: the conn task `stage`s `[Headers, Body?, Close]` once via
/// [`H2Connection::submit_send`][submit] and waits on `completion_waker` for `submit_resolved` to
/// flip. The driver drains the queue into its private send cursor, frames the parts, and on the
/// queue draining to empty stores `completion_result`, sets `submit_resolved = true`, and wakes the
/// conn task.
///
/// **Extended-CONNECT upgrade path** ([RFC 8441]): the conn task `stage`s `[Headers, Body?]` with
/// *no* `Close`, so the stream stays open after the prelude frames. `submit_resolved` flips when
/// the queue first drains (i.e. once the prelude is on the wire) — matching h1/h3, not at
/// `END_HEADERS` — so `submit_upgrade().await` returns and the runtime can dispatch
/// [`Handler::upgrade`][trillium::Handler::upgrade]. The handler then writes through
/// [`H2Transport`]'s `AsyncWrite`, which appends to `outbound`; the send pump drains that ring into
/// DATA. `poll_close` enqueues [`OutboundPart::Close`], the pump drains the ring, frames the
/// terminator, and tears the stream down.
///
/// [submit]: super::H2Connection::submit_send
/// [RFC 8441]: https://www.rfc-editor.org/rfc/rfc8441
#[derive(Debug, Default)]
pub(super) struct SendState {
    /// Outbound parts the conn task has handed off, drained by the driver into its private send
    /// cursor under `needs_servicing`. Cold: a handful of pushes per stream lifetime.
    pub(super) queue: Mutex<VecDeque<OutboundPart>>,

    /// Streaming bytes for a bidirectional upgrade: [`H2Transport`]'s `AsyncWrite::poll_write`
    /// appends here, the send pump drains it into DATA frames before framing a terminator. A
    /// single reused ring (`ignore_front` at head) — empty for normal responses, which frame an
    /// owned [`OutboundPart::Body`] directly.
    pub(super) outbound: Mutex<Buffer>,

    /// Reverse-direction backpressure waker: registered by [`H2Transport::poll_write`] when
    /// `outbound` hits the cap, fired by the send pump after it drains bytes so a parked writer
    /// can resume. Without it a slow/unresponsive peer's closed window would let `outbound`
    /// grow unbounded.
    pub(super) outbound_write_waker: AtomicWaker,

    /// Set to `true` by the driver once the conn task's [`SubmitSend`][super::SubmitSend] future
    /// may resolve and its `completion_result` is readable. The future polls this atomic and
    /// registers on `completion_waker`. Flips when the outbound queue first drains to empty —
    /// `END_STREAM` for a normal response, the prelude-handoff for an upgrade.
    pub(super) submit_resolved: AtomicBool,

    /// The driver writes the final result here before flipping `submit_resolved`. The conn task
    /// takes it once `submit_resolved` is observed true.
    pub(super) completion_result: Mutex<Option<io::Result<()>>>,

    /// The conn task's waker, registered by `SubmitSend::poll` and fired by the driver after
    /// `submit_resolved` is set.
    pub(super) completion_waker: AtomicWaker,

    /// Set by [`H2Transport::Drop`] when the FSM is already `Closed` and the application is
    /// releasing a stream that lingered in the map for post-EOF access (client role). The driver
    /// picks it up and removes the entry from both maps. Not protocol state — a local
    /// resource-ownership fact the FSM can't represent.
    pub(super) transport_dropped: AtomicBool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HttpContext;

    #[test]
    fn request_close_enqueues_single_close() {
        let state = StreamState::default();
        *state.fsm_lock() = StreamFsm::Open;
        state.request_close();
        state.request_close();
        let queue = state.send.queue.lock().unwrap();
        assert_eq!(queue.len(), 1, "second request_close is a no-op");
        assert!(matches!(queue.front(), Some(OutboundPart::Close)));
    }

    #[test]
    fn request_close_noop_when_send_closed() {
        let state = StreamState::default();
        *state.fsm_lock() = StreamFsm::HalfClosedLocal;
        state.request_close();
        assert!(
            state.send.queue.lock().unwrap().is_empty(),
            "no terminator queued once the send half is closed"
        );
    }

    #[test]
    fn request_reset_clears_queue_and_is_first_wins() {
        let state = StreamState::default();
        *state.fsm_lock() = StreamFsm::Open;
        state.stage([OutboundPart::Body(Body::default()), OutboundPart::Close]);
        state.request_reset(H2ErrorCode::Cancel);
        state.request_reset(H2ErrorCode::InternalError);
        let queue = state.send.queue.lock().unwrap();
        assert_eq!(queue.len(), 1, "queue cleared, single reset");
        assert!(
            matches!(
                queue.front(),
                Some(OutboundPart::Reset(H2ErrorCode::Cancel))
            ),
            "first reset code wins",
        );
    }

    #[test]
    fn poll_write_caps_at_response_buffer_max_len() {
        use futures_lite::AsyncWrite;
        use std::task::{Context, Poll, Waker};
        let mut context = HttpContext::new();
        context.config.response_buffer_max_len = 16;
        let connection = H2Connection::new(Arc::new(context));
        let state = Arc::new(StreamState::default());
        *state.fsm_lock() = StreamFsm::Open;
        let mut transport = H2Transport::new(connection, 1, state);
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let buf = [0u8; 32];
        match Pin::new(&mut transport).poll_write(&mut cx, &buf) {
            Poll::Ready(Ok(n)) => assert_eq!(n, 16, "should accept exactly cap bytes"),
            other => panic!("expected Ready(Ok(16)), got {other:?}"),
        }
    }
}
