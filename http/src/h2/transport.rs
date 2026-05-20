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
//!   bytes through the transport's `AsyncRead` via
//!   [`ReceivedBody::handle_raw`][crate::ReceivedBody::handle_raw]. Response bytes flow through
//!   [`H2Connection::submit_send`][submit_send] to the driver's send pump, which frames HEADERS +
//!   DATA + trailing HEADERS onto the connection without ever touching this `AsyncWrite`.
//!
//! - **Extended-CONNECT upgrades** ([RFC 8441] WebSocket-over-h2, plus the in-progress
//!   `draft-ietf-webtrans-http2` for WebTransport-over-h2): after the handler responds 200 to a
//!   `CONNECT` request with a `:protocol` pseudo-header, [`Conn::send_h2`][crate::Conn::send_h2]
//!   routes through [`H2Connection::submit_upgrade`][submit_upgrade] which frames HEADERS without
//!   `END_STREAM`, signals send completion early, and leaves the stream open as a bidirectional
//!   byte channel. The runtime adapter then dispatches
//!   [`Handler::upgrade`][trillium::Handler::upgrade], which gets an [`Upgrade`][crate::Upgrade]
//!   wrapping this transport. `AsyncWrite::poll_write` appends to a per-stream outbound queue
//!   ([`SendState::outbound`]); the driver's send pump drains it into DATA frames bounded by the
//!   per-stream and connection send windows. `AsyncWrite::poll_close` flips
//!   [`SendState::outbound_close_requested`] so the driver eventually emits `DATA(END_STREAM)` and
//!   tears the stream down.
//!
//! [`H2Driver::next`]: super::H2Driver::next
//! [`H2Connection`]: super::H2Connection
//! [`BoxedTransport`]: crate::transport::BoxedTransport
//! [submit_send]: super::H2Connection::submit_send
//! [submit_upgrade]: super::H2Connection::submit_upgrade
//! [RFC 8441]: https://www.rfc-editor.org/rfc/rfc8441

use super::{H2Connection, H2ErrorCode};
use crate::{
    Body, Buffer, Headers,
    headers::hpack::{FieldSection, PseudoHeaders},
};
use atomic_waker::AtomicWaker;
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    fmt, io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll},
};

/// A single HTTP/2 stream's transport handle.
///
/// Carries a backref to the shared [`H2Connection`], the stream id, and the per-stream
/// `Arc<StreamState>` used by the read side. Normal HTTP/2 operation reads through
/// [`ReceivedBody`][crate::ReceivedBody] and writes through the connection's send queue;
/// the `AsyncRead` / `AsyncWrite` impls here are only reached by code that borrows the
/// transport directly (typically an upgrade handler after extended CONNECT).
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
    /// Application-side release / cancel signal, depending on stream state:
    ///
    /// - **Wire-closed cleanly** (`send.completed && recv.eof`): the application is done with a
    ///   stream that already finished on the wire. The client-role lifecycle keeps such streams in
    ///   the map after wire-close (see [`H2Driver::try_close_if_both_done`][super::H2Driver]) so
    ///   the application's transport handle remains valid for trailer / late-read access. Dropping
    ///   the transport is the signal that the application is done, and we forward it to the
    ///   connection so the driver removes the entry from both maps.
    ///
    /// - **Wire-incomplete** (handler panic, conn task abandoned, client awaiting a response that
    ///   never came): emit `RST_STREAM(Cancel)` so the peer learns we're abandoning the stream.
    ///   Without this the leak persists until the entire connection tears down. Symmetric for both
    ///   roles.
    ///
    /// - **Already gone from the shared map**: driver beat us to cleanup; no-op.
    ///
    /// - **Upgrade path graceful close in flight** (`outbound_close_requested`): user has already
    ///   asked for graceful close via [`Self::poll_close`]; the driver is draining the outbound
    ///   queue + emitting `DATA(END_STREAM)`. Don't RST in that window.
    fn drop(&mut self) {
        // Cheap pre-check: if the stream is no longer in the shared map the driver has
        // already cleaned up; nothing to do.
        if !self.connection.streams_lock().contains_key(&self.stream_id) {
            log::trace!(
                "h2 stream {}: H2Transport dropped on already-released stream",
                self.stream_id,
            );
            return;
        }

        let send_done = self.state.send.completed.load(Ordering::Acquire);
        let recv_done = self.state.recv.eof.load(Ordering::Acquire);

        // Both halves wire-closed: release the held entry regardless of whether
        // `poll_close` was called. The connection-pool return path calls `poll_close`
        // on routine cleanup, which sets `outbound_close_requested` on streams that
        // have already completed normally; we must still release rather than treat the
        // flag as "graceful close in flight."
        if send_done && recv_done {
            log::trace!(
                "h2 stream {}: H2Transport dropped on wire-closed stream — releasing",
                self.stream_id,
            );
            self.connection.release_stream(self.stream_id);
            return;
        }

        // Upgrade lifecycle: `mark_drop_graceful` has flipped the per-stream flag,
        // signaling that this transport has crossed into user code and a drop means
        // "done" rather than "handler panicked." Schedule graceful close so the
        // driver drains outbound bytes and emits END_STREAM rather than
        // RST_STREAM(Cancel). If the flag is *already* set the driver is already
        // draining; just leave it running.
        if self.state.send.graceful_drop.load(Ordering::Acquire) {
            if self
                .state
                .send
                .outbound_close_requested
                .swap(true, Ordering::AcqRel)
            {
                log::trace!(
                    "h2 stream {}: H2Transport dropped with graceful close already in flight — \
                     letting driver finish",
                    self.stream_id,
                );
                return;
            }
            log::trace!(
                "h2 stream {}: H2Transport dropped (upgrade) — scheduling graceful close \
                 (send_done={send_done}, recv_done={recv_done})",
                self.stream_id,
            );
            self.state.needs_servicing.store(true, Ordering::Release);
            self.state.send.outbound_waker.wake();
            self.connection.outbound_waker().wake();
            return;
        }

        // Mid-stream drop on a non-upgrade stream: RST so the peer learns we're done.
        log::debug!(
            "h2 stream {}: H2Transport dropped mid-stream — RST_STREAM(Cancel) \
             (send_done={send_done}, recv_done={recv_done})",
            self.stream_id,
        );
        self.connection
            .stream_error(self.stream_id, H2ErrorCode::Cancel);
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
            // Drop the buf lock before the waker fire so the driver can grab it without
            // contention when it wakes.
            drop(recv);
            // Tell the driver how many bytes the handler consumed so it can emit a matching
            // `WINDOW_UPDATE` and keep the peer's stream + connection windows topped up.
            // `fetch_add` accumulates across calls that happen before the driver's next
            // service tick; the driver's `swap(0)` takes the whole batch at once.
            recv_state
                .bytes_consumed
                .fetch_add(take as u64, Ordering::AcqRel);
            self.state.needs_servicing.store(true, Ordering::Release);
            connection.outbound_waker().wake();
            return Poll::Ready(Ok(take));
        }

        // Buffer empty. EOF if END_STREAM was observed, otherwise register and wait.
        // The driver acquires the same `buf` lock to push data and to set `eof`, so holding
        // it here is enough to make the eof check final — no register-then-check race window
        // between us and the driver's wake.
        if recv_state.eof.load(Ordering::Acquire) {
            return Poll::Ready(Ok(0));
        }
        recv_state.waker.register(cx.waker());
        Poll::Pending
    }
}

impl AsyncWrite for H2Transport {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        // Append into the per-stream outbound queue used by the extended-CONNECT
        // (RFC 8441) upgrade path. The driver's send pump drains the same queue (via
        // the upgrade body's `AsyncRead::poll_read`) into DATA frames bounded by
        // per-stream + connection send windows.
        //
        // Bounded by `config.response_buffer_max_len` — the same cap h1 and h3 response
        // paths use for their transit buffers. If the peer's flow-control window stalls
        // (slow or malicious reader) the driver can't drain `outbound`, the cap is hit,
        // and we return `Pending` so the handler is throttled. The drain side
        // (`H2OutboundReader::poll_read`) wakes `outbound_write_waker` after each take.
        let send = &self.state.send;

        if send.outbound_close_requested.load(Ordering::Acquire) {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }

        let cap = self.connection.context.config.response_buffer_max_len;
        let mut outbound = send.outbound.lock().expect("outbound buf mutex poisoned");
        if outbound.len() >= cap {
            // Register first, then re-check under lock to close the race against the
            // drain side (`H2OutboundReader::poll_read` takes the same lock to call
            // `ignore_front` and then wakes us). If a drain landed between our length
            // check and the register, the second check sees the freed space.
            send.outbound_write_waker.register(cx.waker());
            if outbound.len() >= cap {
                return Poll::Pending;
            }
        }
        let take = (cap - outbound.len()).min(buf.len());
        log::trace!(
            "h2 stream {}: H2Transport::poll_write appending {take}/{} bytes to outbound queue",
            self.stream_id,
            buf.len(),
        );
        outbound.extend_from_slice(&buf[..take]);
        drop(outbound);

        // Wake the driver task (if parked on the connection-level waker) and the
        // upgrade body's poll_read (in case it's registered between driver ticks).
        // Firing both is cheap and resolves the cross-task race where the driver
        // happens to be parked on `connection.outbound_waker` rather than mid-body-poll.
        send.outbound_waker.wake();
        self.connection.outbound_waker().wake();
        Poll::Ready(Ok(take))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Best-effort: bytes appended via `poll_write` are already visible to the driver
        // and will be framed on the next tick. There's no application-level "flushed"
        // state below us to wait on.
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Mark the upgrade write-half closed. Once the driver drains the remaining
        // outbound bytes, the upgrade body's `poll_read` will return `Ready(0)`, the
        // send pump transitions through trailers (none) into `DATA(END_STREAM)`, and the
        // stream tears down via the normal `complete_and_remove_stream` path.
        log::trace!(
            "h2 stream {}: H2Transport::poll_close marking outbound closed",
            self.stream_id,
        );
        self.state
            .send
            .outbound_close_requested
            .store(true, Ordering::Release);
        self.state.send.outbound_waker.wake();
        self.connection.outbound_waker().wake();
        Poll::Ready(Ok(()))
    }
}

/// Shared per-stream state. Owned by an [`Arc`] held jointly by the driver (via the connection's
/// stream table) and the handler task (via [`H2Transport`]).
#[derive(Debug, Default)]
pub(super) struct StreamState {
    /// Recv side: inbound DATA payloads, EOF flag, handler waker, handler-intent signal.
    pub(super) recv: RecvState,
    /// Send side: handoff slot from the conn task's `submit_send`, plus completion signaling
    /// the conn task awaits.
    pub(super) send: SendState,
    /// Stream-error request raised from the conn-task side. Populated by
    /// [`H2Connection::stream_error`][super::H2Connection::stream_error] when something on
    /// the conn-task side (a body-read that detects content-length mismatch, a handler
    /// that wants to abort) needs the driver to emit `RST_STREAM` and clean up. The driver
    /// picks this up in `service_handler_signals` on its next tick and routes through
    /// [`H2Driver::complete_and_remove_stream`][super::H2Driver]'s normal cleanup path.
    ///
    /// [`H2Driver`]: super::H2Driver
    pub(super) pending_reset: Mutex<Option<H2ErrorCode>>,

    /// Client-role: the application has dropped its [`H2Transport`] handle on a stream that
    /// already wire-closed cleanly (both halves observed `END_STREAM`). The driver removes
    /// the stream from both maps on its next `service_handler_signals` tick. No `RST_STREAM`
    /// — the wire-side is already closed; this is purely application-side resource cleanup.
    /// Distinct from [`Self::pending_reset`], which emits `RST_STREAM` for unclean teardown.
    ///
    /// Server role never sets this — server streams are removed eagerly when the response
    /// finishes sending (no held-after-close lifecycle).
    pub(super) pending_release: AtomicBool,

    /// Mailbox flag for conn-task → driver work signaling.
    ///
    /// Set to `true` by conn-task code whenever it produces work the driver should service
    /// (new submission, [`Self::pending_reset`], [`Self::pending_release`], a
    /// [`RecvState::bytes_consumed`] increment, or a [`RecvState::is_reading`] transition).
    /// The driver's `service_handler_signals` walks every stream and consults this flag via
    /// `swap(false, AcqRel)` — only streams where it returns `true` pay for the per-field
    /// pickup (mutex acquires for `submission` / `pending_reset`, etc.). Idle streams cost a
    /// single atomic RMW per tick.
    ///
    /// **Setter ordering rule**: write the underlying state first, then store `true` with
    /// `Release`, then call [`H2Connection::outbound_waker`][super::H2Connection]`.wake()`.
    /// The `Release` store + driver's `AcqRel` swap form the synchronization edge that
    /// publishes the underlying state to the driver. Over-notification (driver clears, finds
    /// nothing, moves on) is harmless; under-notification would lose a signal — which is why
    /// the underlying state must be written *before* the flag store.
    pub(super) needs_servicing: AtomicBool,
}

/// Receive-side per-stream state.
#[derive(Debug, Default)]
pub(super) struct RecvState {
    /// Inbound DATA body bytes awaiting handler read. A single persistent ring (append-at-tail,
    /// `ignore_front`-at-head): the driver appends via `extend_from_slice` when a DATA frame
    /// arrives; the handler reads from the front and virtually drops consumed bytes. When
    /// `ignore_front` catches up to the data end the `Buffer` truncates to zero, so the
    /// underlying `Vec` capacity stays bounded by peak in-flight bytes rather than cumulative
    /// traffic — zero amortized allocations per DATA frame.
    pub(super) buf: Mutex<Buffer>,

    /// `true` once `END_STREAM` has been observed for this stream's recv side. Set by the
    /// driver under the same `buf` lock used for pushes; checked by `poll_read` while
    /// holding that lock to decide between EOF and Pending.
    pub(super) eof: AtomicBool,

    /// Handler-task waker, fired by the driver after pushing DATA into `buf` or after
    /// setting `eof`. Single-waiter: only one task ever polls a given `H2Transport`.
    pub(super) waker: AtomicWaker,

    /// Set by the handler's first [`H2Transport::poll_read`] to declare intent to consume the
    /// request body. The driver observes the transition and emits a `WINDOW_UPDATE` for this
    /// stream, topping its recv window up from `SETTINGS_INITIAL_WINDOW_SIZE` (advertised as
    /// `0`) to the per-stream maximum. Once set, stays set.
    pub(super) is_reading: AtomicBool,

    /// Bytes the handler has consumed from `buf` since the driver last sampled this counter.
    /// Incremented by [`H2Transport::poll_read`] using `fetch_add` after each drain; the
    /// driver reads it via `swap(0)` on each tick and emits stream-level + connection-level
    /// `WINDOW_UPDATE` for the consumed total. Ensures a handler draining a body larger than
    /// a single window doesn't stall the peer.
    pub(super) bytes_consumed: AtomicU64,

    /// Trailers, populated by the driver if a trailing HEADERS frame arrives for this stream.
    /// Always written *before* `eof` is set, so once the handler observes `Ready(0)` on the
    /// recv side, any trailers for this request are guaranteed to be in place.
    ///
    /// Taken out and moved into [`Conn::request_trailers`][crate::Conn] by the receiver-side
    /// body state machine when it transitions to
    /// [`ReceivedBodyState::End`][crate::received_body::ReceivedBodyState].
    pub(super) trailers: Mutex<Option<Headers>>,

    /// Client-role: response HEADERS field section, populated by the driver on the first
    /// non-1xx HEADERS frame arrival for a client-initiated stream. Server role doesn't use
    /// this slot (response HEADERS go *out* on the server, not in). Single-shot: the conn
    /// task takes the `FieldSection` via [`H2Connection::response_headers`][super::H2Connection]
    /// once; subsequent HEADERS arrivals on the same stream are interpreted as trailers and
    /// routed to the [`Self::trailers`] slot. Interim 1xx HEADERS frames are discarded by
    /// the driver without touching this slot or latching `first_response_headers_seen`.
    pub(super) response_headers: Mutex<Option<FieldSection<'static>>>,

    /// Client-role: latching flag for "first HEADERS arrived for this stream." Distinct from
    /// `response_headers.is_some()` — the conn task drains that slot when it consumes
    /// headers, so the driver can't use slot occupancy to distinguish "haven't seen
    /// HEADERS yet" from "headers seen + already taken." Set inside `finalize_response_headers`
    /// before that slot is populated; checked by `route_headers` on subsequent HEADERS to
    /// route them as trailers. Never cleared.
    pub(super) first_response_headers_seen: AtomicBool,

    /// Client-role: waker the conn task registers via
    /// [`H2Connection::response_headers`][super::H2Connection]; fired by the driver after
    /// stashing the `FieldSection` in [`Self::response_headers`] *or* on stream removal (so
    /// a parked conn task observing the stream gone surfaces `NotConnected` instead of
    /// hanging).
    pub(super) response_headers_waker: AtomicWaker,
}

/// Send-side per-stream state used to hand a response from the conn task to the driver,
/// plus the outbound byte queue for extended-CONNECT upgraded streams.
///
/// **Normal response path**: the conn task fills `submission` once via
/// [`H2Connection::submit_send`][submit] and waits on `completion_waker` for `completed` to
/// flip. The driver picks up the submission on its next `drive` tick, frames it (HEADERS,
/// DATA, optional trailing HEADERS) into the connection's outbound buffer as send-side flow
/// control allows, and on completion stores the `completion_result`, sets `completed = true`,
/// and wakes the conn task.
///
/// **Extended-CONNECT upgrade path** ([RFC 8441]): the conn task calls
/// [`H2Connection::submit_upgrade`][submit_upgrade], which constructs an
/// [`H2OutboundReader`] over `outbound` / `outbound_close_requested` /
/// `outbound_waker` and submits it as the response body. The driver signals
/// `completion_waker` as soon as the response HEADERS frame is on the wire (instead of
/// waiting for the body to drain), so the conn task's `submit_upgrade().await` returns and
/// the runtime adapter can dispatch [`Handler::upgrade`][trillium::Handler::upgrade]. The
/// upgrade handler then writes through [`H2Transport`]'s `AsyncWrite`, which appends to
/// `outbound`; the driver's send pump pulls those bytes out via the body's `AsyncRead`
/// and frames them as DATA. Closing the transport sets `outbound_close_requested`, the
/// reader returns `Ready(0)`, and the send pump terminates the stream with
/// `DATA(END_STREAM)`.
///
/// [submit]: super::H2Connection::submit_send
/// [submit_upgrade]: super::H2Connection::submit_upgrade
/// [RFC 8441]: https://www.rfc-editor.org/rfc/rfc8441
#[derive(Debug, Default)]
pub(super) struct SendState {
    /// Slot for the conn task's submission. Some between `submit_send` and the driver's
    /// pickup tick; None at all other times.
    pub(super) submission: Mutex<Option<Submission>>,

    /// Set to `true` by the driver once the response has been fully framed, flushed, or
    /// errored. The conn task's `SubmitSend` future polls this atomic and registers on
    /// `completion_waker`.
    pub(super) completed: AtomicBool,

    /// The driver writes the final result here before flipping `completed`. The conn task
    /// takes it once `completed` is observed true.
    pub(super) completion_result: Mutex<Option<io::Result<()>>>,

    /// The conn task's waker, registered by `SubmitSend::poll` and fired by the driver
    /// after `completed` is set.
    pub(super) completion_waker: AtomicWaker,

    /// Outbound bytes for an extended-CONNECT (RFC 8441) upgraded stream.
    /// Appended to by [`H2Transport`]'s `AsyncWrite::poll_write` and drained by the
    /// upgrade body's `AsyncRead::poll_read` (the driver-task side of the send pump).
    /// Empty for normal responses — the driver pumps the response [`Body`] directly.
    pub(super) outbound: Mutex<Buffer>,

    /// Set by [`H2Transport::poll_close`] to mark the upgrade-side write half closed.
    /// The upgrade body's `poll_read` returns `Ready(0)` once `outbound` is empty and
    /// this flag is set, which transitions the driver's send pump into the
    /// trailers/`DATA(END_STREAM)` phase.
    pub(super) outbound_close_requested: AtomicBool,

    /// Waker for the upgrade body's `poll_read`. Fired by [`H2Transport::poll_write`]
    /// after appending bytes and by [`H2Transport::poll_close`] after flipping
    /// `outbound_close_requested`. Registered by the body during its `poll_read` when
    /// it observes an empty buffer and no close flag.
    pub(super) outbound_waker: AtomicWaker,

    /// Reverse-direction waker: registered by [`H2Transport::poll_write`] when `outbound`
    /// has reached the configured cap, fired by [`H2OutboundReader::poll_read`] after it
    /// drains bytes (i.e. after `ignore_front`) so a parked writer can resume. This is the
    /// edge that surfaces peer flow-control backpressure to the upgrade handler — without
    /// it, a slow or unresponsive peer's closed window would let `outbound` grow without
    /// bound.
    pub(super) outbound_write_waker: AtomicWaker,

    /// Mailbox for trailers staged out-of-band by
    /// [`H2Connection::submit_trailers`][super::H2Connection::submit_trailers]. The
    /// driver moves these onto the send cursor when it next services the stream.
    pub(super) pending_trailers: Mutex<Option<Headers>>,

    /// When `true`, a mid-stream [`H2Transport`] drop schedules graceful close instead
    /// of `RST_STREAM(Cancel)`. Flipped by
    /// [`H2Connection::mark_drop_graceful`][super::H2Connection::mark_drop_graceful]
    /// once stream ownership has crossed into user code.
    pub(super) graceful_drop: AtomicBool,
}

/// `AsyncRead` source the driver uses as the response body for an extended-CONNECT upgrade.
///
/// Reads from [`SendState::outbound`] — the same per-stream queue [`H2Transport`]'s
/// `AsyncWrite::poll_write` appends to. Returns `Ready(0)` once the queue is empty and
/// [`SendState::outbound_close_requested`] has been set (handler dropped or called
/// `poll_close` on the transport), at which point the driver's send pump transitions
/// through trailers (none) into `DATA(END_STREAM)` and tears the stream down.
///
/// Constructed by [`H2Connection::submit_upgrade`][super::H2Connection::submit_upgrade];
/// wrapped in [`Body::new_streaming`] so the existing send pump can pump it as if it were
/// any other unknown-length response body.
#[derive(Debug)]
pub(super) struct H2OutboundReader {
    state: Arc<StreamState>,
    stream_id: u32,
}

impl H2OutboundReader {
    pub(super) fn new(state: Arc<StreamState>, stream_id: u32) -> Self {
        Self { state, stream_id }
    }
}

impl AsyncRead for H2OutboundReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if out.is_empty() {
            return Poll::Ready(Ok(0));
        }

        let send = &self.state.send;
        let mut outbound = send.outbound.lock().expect("outbound buf mutex poisoned");
        let take = out.len().min(outbound.len());
        if take > 0 {
            out[..take].copy_from_slice(&outbound[..take]);
            outbound.ignore_front(take);
            log::trace!(
                "h2 stream {}: H2OutboundReader::poll_read drained {take} bytes",
                self.stream_id,
            );
            // Drop the lock before waking — the writer reacquires it on resume.
            drop(outbound);
            // Surface flow-control backpressure: wake any writer parked on
            // `outbound_write_waker` because the cap was hit. Registered-but-still-full
            // is harmless — the writer's recheck under lock observes the new len.
            send.outbound_write_waker.wake();
            return Poll::Ready(Ok(take));
        }

        // Queue empty. Register first, then re-check the close flag. This closes the
        // register-then-check race against `poll_close` (which doesn't take the buf
        // lock — it just stores the flag and fires the waker). Holding the buf lock
        // means `poll_write` can't race here; only `poll_close` can.
        send.outbound_waker.register(cx.waker());

        if send.outbound_close_requested.load(Ordering::Acquire) {
            log::trace!(
                "h2 stream {}: H2OutboundReader::poll_read EOF (close_requested + empty)",
                self.stream_id,
            );
            return Poll::Ready(Ok(0));
        }
        Poll::Pending
    }
}

/// What the conn task hands the driver to begin a send on a stream.
///
/// `body` carries either a normal response body or, for extended-CONNECT (RFC 8441)
/// upgrades, a streaming body that reads from [`SendState::outbound`] (which the upgrade
/// handler's [`H2Transport`] `AsyncWrite` writes into). Trailers (if any) come from
/// [`Body::trailers`] after drain — not a separate field.
///
/// `is_upgrade` flips the driver's completion semantics: instead of signaling
/// [`SubmitSend`][super::SubmitSend] completion after the body is fully on the wire, the
/// driver signals completion as soon as the response HEADERS frame is flushed. That lets
/// [`Conn::send_h2`][crate::Conn::send_h2] return so the runtime can dispatch
/// [`Handler::upgrade`][trillium::Handler::upgrade], while the body keeps streaming in the
/// background.
#[derive(Debug)]
pub(super) struct Submission {
    /// Owned pseudo-headers for the response/request. Combined with `headers` on the driver
    /// task to form a [`FieldSection`] which is then HPACK-encoded synchronously via
    /// [`HpackEncoder`][crate::headers::hpack::HpackEncoder] at submission pickup. The
    /// encoder runs only on the driver task: each pickup-tick encodes its submissions
    /// against the live dynamic-table state, then frames HEADERS in the order they were
    /// encoded — matching the wire-emission order that HPACK's stateful decoder requires.
    pub(super) pseudos: PseudoHeaders<'static>,
    /// Owned headers for the block. Cloned from the conn task's `request_headers` /
    /// `response_headers` so those remain readable to caller and middleware after the send.
    pub(super) headers: Headers,
    pub(super) body: Option<Body>,
    pub(super) is_upgrade: bool,
}

impl Submission {
    /// Borrow this submission's headers as a [`FieldSection`] for encoding.
    pub(super) fn field_section(&self) -> FieldSection<'_> {
        FieldSection::new(self.pseudos.clone(), &self.headers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HttpContext;
    use futures_lite::{AsyncRead, AsyncWrite};
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        task::{Context, Poll, Wake, Waker},
    };

    struct CountingWaker(AtomicBool);
    impl Wake for CountingWaker {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::Release);
        }
    }

    fn pair_with_cap(cap: usize) -> (H2Transport, H2OutboundReader) {
        let mut context = HttpContext::new();
        context.config.response_buffer_max_len = cap;
        let connection = H2Connection::new(Arc::new(context));
        let state = Arc::new(StreamState::default());
        let transport = H2Transport::new(connection.clone(), 1, state.clone());
        let reader = H2OutboundReader::new(state, 1);
        (transport, reader)
    }

    #[test]
    fn poll_write_caps_at_response_buffer_max_len() {
        // Cap of 16 bytes. Writing 32 bytes should accept exactly 16 (partial-write
        // semantics; AsyncWriteExt::write_all retries the rest).
        let (mut transport, _reader) = pair_with_cap(16);
        let waker = Waker::from(Arc::new(CountingWaker(AtomicBool::new(false))));
        let mut cx = Context::from_waker(&waker);

        let buf = [0u8; 32];
        match Pin::new(&mut transport).poll_write(&mut cx, &buf) {
            Poll::Ready(Ok(n)) => assert_eq!(n, 16, "should accept exactly cap bytes"),
            other => panic!("expected Ready(Ok(16)), got {other:?}"),
        }
    }

    #[test]
    fn poll_write_returns_pending_when_full_and_drain_wakes() {
        let (mut transport, mut reader) = pair_with_cap(8);
        let counting = Arc::new(CountingWaker(AtomicBool::new(false)));
        let writer_waker = Waker::from(counting.clone());
        let mut writer_cx = Context::from_waker(&writer_waker);

        // Fill the buffer to the cap.
        let buf = [0u8; 8];
        match Pin::new(&mut transport).poll_write(&mut writer_cx, &buf) {
            Poll::Ready(Ok(8)) => {}
            other => panic!("expected Ready(Ok(8)), got {other:?}"),
        }

        // Next write must return Pending — buffer is at cap.
        let extra = [0u8; 4];
        match Pin::new(&mut transport).poll_write(&mut writer_cx, &extra) {
            Poll::Pending => {}
            other => panic!("expected Pending, got {other:?}"),
        }
        assert!(
            !counting.0.load(Ordering::Acquire),
            "writer waker should not have fired yet"
        );

        // Drain via the reader — this should wake the writer.
        let reader_waker = Waker::noop().clone();
        let mut reader_cx = Context::from_waker(&reader_waker);
        let mut sink = [0u8; 4];
        match Pin::new(&mut reader).poll_read(&mut reader_cx, &mut sink) {
            Poll::Ready(Ok(4)) => {}
            other => panic!("expected Ready(Ok(4)), got {other:?}"),
        }
        assert!(
            counting.0.load(Ordering::Acquire),
            "drain should have woken the writer"
        );

        // Re-poll the writer — there's now room for the 4 extra bytes.
        match Pin::new(&mut transport).poll_write(&mut writer_cx, &extra) {
            Poll::Ready(Ok(4)) => {}
            other => panic!("expected Ready(Ok(4)), got {other:?}"),
        }
    }

    /// `pair_with_cap` variant that also inserts the per-stream [`StreamState`] into the
    /// connection's streams map, so `H2Connection::submit_*` methods see the stream as
    /// registered. Returns the [`H2Connection`] for direct API access too.
    #[cfg(feature = "unstable")]
    fn registered_pair(
        cap: usize,
        stream_id: u32,
    ) -> (Arc<H2Connection>, H2Transport, H2OutboundReader) {
        let mut context = HttpContext::new();
        context.config.response_buffer_max_len = cap;
        let connection = H2Connection::new(Arc::new(context));
        let state = Arc::new(StreamState::default());
        connection.streams_lock().insert(stream_id, state.clone());
        let transport = H2Transport::new(connection.clone(), stream_id, state.clone());
        let reader = H2OutboundReader::new(state, stream_id);
        (connection, transport, reader)
    }

    #[cfg(feature = "unstable")]
    #[test]
    fn submit_trailers_stages_pending_trailers_and_flips_close() {
        use crate::Headers;

        let stream_id = 1;
        let (connection, _transport, _reader) = registered_pair(64, stream_id);

        let mut trailers = Headers::new();
        trailers.insert("grpc-status", "0");
        trailers.insert("grpc-message", "OK");

        connection
            .submit_trailers(stream_id, trailers)
            .expect("submit_trailers on a registered stream");

        let state = connection
            .streams_lock()
            .get(&stream_id)
            .cloned()
            .expect("stream still in map");

        let pending = state
            .send
            .pending_trailers
            .lock()
            .expect("pending_trailers mutex");
        let pending = pending.as_ref().expect("pending_trailers populated");
        assert_eq!(pending.get_str("grpc-status"), Some("0"));
        assert_eq!(pending.get_str("grpc-message"), Some("OK"));

        assert!(
            state.send.outbound_close_requested.load(Ordering::Acquire),
            "submit_trailers must flip outbound_close_requested"
        );
        assert!(
            state.needs_servicing.load(Ordering::Acquire),
            "submit_trailers must raise needs_servicing"
        );
    }

    #[cfg(feature = "unstable")]
    #[test]
    fn submit_trailers_returns_not_connected_for_unknown_stream() {
        use crate::Headers;
        use std::io;

        let connection = H2Connection::new(Arc::new(HttpContext::new()));
        let err = connection
            .submit_trailers(99, Headers::new())
            .expect_err("unknown stream should be NotConnected");
        assert_eq!(err.kind(), io::ErrorKind::NotConnected);
    }

    #[cfg(feature = "unstable")]
    #[test]
    fn submit_trailers_wakes_outbound_reader_to_eof() {
        // Once `submit_trailers` flips close_requested + the outbound queue is empty, the
        // `H2OutboundReader` should observe EOF (`Ready(Ok(0))`) — that's the signal the
        // driver's send pump needs to transition the cursor from Body → Trailers and
        // ultimately emit the trailing HEADERS frame.
        use crate::Headers;

        let stream_id = 1;
        let (connection, _transport, mut reader) = registered_pair(64, stream_id);

        connection
            .submit_trailers(stream_id, Headers::new())
            .expect("submit_trailers");

        let waker = Waker::from(Arc::new(CountingWaker(AtomicBool::new(false))));
        let mut cx = Context::from_waker(&waker);
        let mut sink = [0u8; 4];
        match Pin::new(&mut reader).poll_read(&mut cx, &mut sink) {
            Poll::Ready(Ok(0)) => {}
            other => panic!("expected Ready(Ok(0)) (EOF), got {other:?}"),
        }
    }

    /// Regression: while a stream still lives inside a `Conn` handler, dropping the
    /// `H2Transport` mid-stream signals "handler panicked" — the driver must RST so the
    /// peer learns the work is cancelled. `mark_drop_graceful` has *not* been called, so
    /// drop falls into the RST branch.
    #[cfg(feature = "unstable")]
    #[test]
    fn drop_in_conn_state_rsts_mid_stream() {
        use crate::h2::H2ErrorCode;

        let stream_id = 1;
        let (connection, transport, _reader) = registered_pair(64, stream_id);
        let state = connection
            .streams_lock()
            .get(&stream_id)
            .cloned()
            .expect("stream registered");

        drop(transport);

        assert!(
            !state.send.outbound_close_requested.load(Ordering::Acquire),
            "Conn-state drop must not flip outbound_close_requested",
        );
        let pending_reset = state
            .pending_reset
            .lock()
            .expect("pending_reset mutex poisoned");
        assert_eq!(
            *pending_reset,
            Some(H2ErrorCode::Cancel),
            "Conn-state drop must schedule RST_STREAM(Cancel)",
        );
    }

    /// Regression: once `mark_drop_graceful` flips the per-stream flag (the
    /// `Conn → Upgrade` transition), dropping the `H2Transport` schedules graceful
    /// close instead — the driver drains pending outbound bytes and emits
    /// `DATA(END_STREAM)` on its normal complete-and-remove path, no RST.
    #[cfg(feature = "unstable")]
    #[test]
    fn drop_after_mark_drop_graceful_schedules_close() {
        let stream_id = 1;
        let (connection, transport, _reader) = registered_pair(64, stream_id);
        let state = connection
            .streams_lock()
            .get(&stream_id)
            .cloned()
            .expect("stream registered");

        connection.mark_drop_graceful(stream_id);
        drop(transport);

        assert!(
            state.send.outbound_close_requested.load(Ordering::Acquire),
            "graceful drop must flip outbound_close_requested",
        );
        assert!(
            state.needs_servicing.load(Ordering::Acquire),
            "graceful drop must raise needs_servicing so the driver picks the stream up",
        );
        let pending_reset = state
            .pending_reset
            .lock()
            .expect("pending_reset mutex poisoned");
        assert!(
            pending_reset.is_none(),
            "graceful drop must not schedule RST",
        );
    }

    /// Regression: dropping the `H2Transport` on a wire-closed stream
    /// (`send.completed && recv.eof`) must release the held entry **regardless** of
    /// whether `outbound_close_requested` is set. The pool-return path calls `poll_close`
    /// on streams that have already completed normally — setting that flag — and prior
    /// to the Drop reordering fix the early-return on `outbound_close_requested` would
    /// suppress the release, leaving the stream pinned in the connection's map and
    /// blocking `Closing → Drained` for the duration of the held conn.
    #[cfg(feature = "unstable")]
    #[test]
    fn drop_on_wire_closed_with_close_requested_still_releases() {
        let stream_id = 1;
        let (connection, transport, _reader) = registered_pair(64, stream_id);
        let state = connection
            .streams_lock()
            .get(&stream_id)
            .cloned()
            .expect("stream registered");

        // Simulate the wire-closed state both halves reach at the end of a normal
        // request: server's response framed (send.completed), peer's END_STREAM observed
        // (recv.eof).
        state.send.completed.store(true, Ordering::Release);
        state.recv.eof.store(true, Ordering::Release);
        // Simulate the pool-return path calling `poll_close` on the transport's
        // `AsyncWrite` half before drop — this is routine cleanup, not an upgrade-style
        // graceful close.
        state
            .send
            .outbound_close_requested
            .store(true, Ordering::Release);

        drop(transport);

        assert!(
            state.pending_release.load(Ordering::Acquire),
            "Drop on wire-closed stream must signal pending_release even when \
             outbound_close_requested was set by routine cleanup",
        );
        let pending_reset = state
            .pending_reset
            .lock()
            .expect("pending_reset mutex poisoned");
        assert!(
            pending_reset.is_none(),
            "Drop on wire-closed stream must not schedule RST",
        );
    }

    /// Regression: dropping the `H2Transport` while an upgrade's graceful close is
    /// already in flight (`graceful_drop` set AND `outbound_close_requested` already set
    /// by a prior `poll_close`) must be a no-op — re-raising `needs_servicing` or
    /// re-firing wakers risks the driver picking up the stream a second time after it
    /// has already begun finalizing the upgrade.
    #[cfg(feature = "unstable")]
    #[test]
    fn drop_with_graceful_close_already_in_flight_is_noop() {
        let stream_id = 1;
        let (connection, transport, _reader) = registered_pair(64, stream_id);
        let state = connection
            .streams_lock()
            .get(&stream_id)
            .cloned()
            .expect("stream registered");

        connection.mark_drop_graceful(stream_id);
        // Simulate `poll_close` having already fired (user explicitly closed the upgrade
        // write half). The driver is already draining outbound + will emit END_STREAM.
        state
            .send
            .outbound_close_requested
            .store(true, Ordering::Release);
        // Clear `needs_servicing` so we can detect whether Drop spuriously re-raises it.
        state.needs_servicing.store(false, Ordering::Release);

        drop(transport);

        assert!(
            !state.needs_servicing.load(Ordering::Acquire),
            "Drop with graceful close already in flight must not re-raise needs_servicing",
        );
        assert!(
            !state.pending_release.load(Ordering::Acquire),
            "Drop with graceful close already in flight must not signal pending_release — the \
             driver is responsible for finalizing the upgrade",
        );
        let pending_reset = state
            .pending_reset
            .lock()
            .expect("pending_reset mutex poisoned");
        assert!(
            pending_reset.is_none(),
            "Drop with graceful close already in flight must not schedule RST",
        );
    }

    /// `mark_drop_graceful` on an unknown stream id silently no-ops — it's called from
    /// the `Conn → Upgrade` transition where the stream may already have been torn down
    /// (e.g. peer RST arrived before the handler finished). Should not panic.
    #[cfg(feature = "unstable")]
    #[test]
    fn mark_drop_graceful_no_ops_for_unknown_stream() {
        let connection = H2Connection::new(Arc::new(HttpContext::new()));
        connection.mark_drop_graceful(99);
    }
}
