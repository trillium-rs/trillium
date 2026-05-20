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

use super::{H2Connection, H2ErrorCode, lifecycle::StreamLifecycle};
use crate::{Buffer, Headers, headers::hpack::FieldSection};
use atomic_waker::AtomicWaker;
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
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
    /// Application-side release / cancel signal. A single flat match on the lifecycle
    /// variant — the variant *is* the answer, so there is no ordering between checks to
    /// get wrong:
    ///
    /// - `Reset` / `ResetRequested`: stream is already torn down or about to be; no-op.
    /// - `AwaitingRelease`: client-only wire-closed-but-held lifecycle. Signal release so the
    ///   driver removes the entry from both maps.
    /// - `UpgradeOpen` / `UpgradeClosing`: extended-CONNECT lifecycle. Schedule graceful close —
    ///   the variant itself records that this stream's drop semantics are "graceful," no separate
    ///   `graceful_drop` flag needed. Wake is idempotent if already in `UpgradeClosing`.
    /// - Anything else (`Idle` / `Submitted` / `Sending`): mid-stream drop on a normal
    ///   request/response — emit `RST_STREAM(Cancel)` so the peer learns we abandoned it.
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

        let mut lifecycle = self.state.lifecycle_lock();
        match &*lifecycle {
            StreamLifecycle::Reset(_) | StreamLifecycle::ResetRequested(_) => {
                log::trace!(
                    "h2 stream {}: H2Transport dropped on already-reset stream",
                    self.stream_id,
                );
            }
            StreamLifecycle::AwaitingRelease => {
                // Shouldn't ordinarily happen — this branch's Drop path is what
                // *transitions* a stream into AwaitingRelease. Reaching it again means
                // a second Drop, which is structurally impossible (`H2Transport` is not
                // `Clone`); leaving it as a defensive no-op.
                log::trace!(
                    "h2 stream {}: H2Transport dropped while already AwaitingRelease",
                    self.stream_id,
                );
            }
            StreamLifecycle::UpgradeOpen { recv_eof } => {
                let recv_eof = *recv_eof;
                log::trace!(
                    "h2 stream {}: H2Transport dropped (upgrade) — scheduling graceful close",
                    self.stream_id,
                );
                *lifecycle = StreamLifecycle::UpgradeClosing {
                    recv_eof,
                    pending_trailers: None,
                };
                drop(lifecycle);
                self.state.needs_servicing.store(true, Ordering::Release);
                self.state.send.outbound_waker.wake();
                self.connection.outbound_waker().wake();
            }
            StreamLifecycle::UpgradeClosing { .. } => {
                log::trace!(
                    "h2 stream {}: H2Transport dropped — graceful close already in flight",
                    self.stream_id,
                );
                // Driver is already draining; just nudge in case it parked.
                drop(lifecycle);
                self.state.needs_servicing.store(true, Ordering::Release);
                self.state.send.outbound_waker.wake();
                self.connection.outbound_waker().wake();
            }
            _ => {
                // Idle / Submitted / Sending: mid-stream drop. Check the wire-closed
                // case first — recv_eof + send.completed means the application is done
                // with a stream that already finished on the wire, and we forward to
                // release (client-role keeps streams in the map past wire-close so
                // late-trailer / late-read access works). Otherwise RST_STREAM(Cancel).
                let send_done = self.state.send.completed.load(Ordering::Acquire);
                let recv_done = lifecycle.recv_eof();
                if send_done && recv_done {
                    log::trace!(
                        "h2 stream {}: H2Transport dropped on wire-closed stream — releasing",
                        self.stream_id,
                    );
                    *lifecycle = StreamLifecycle::AwaitingRelease;
                    drop(lifecycle);
                    self.state.needs_servicing.store(true, Ordering::Release);
                    self.connection.outbound_waker().wake();
                } else {
                    log::debug!(
                        "h2 stream {}: H2Transport dropped mid-stream — RST_STREAM(Cancel)",
                        self.stream_id,
                    );
                    *lifecycle = StreamLifecycle::ResetRequested(H2ErrorCode::Cancel);
                    drop(lifecycle);
                    self.state.needs_servicing.store(true, Ordering::Release);
                    self.connection.outbound_waker().wake();
                }
            }
        }
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

        // Buffer empty. Register the waker *before* releasing the buf lock so a driver
        // push between this poll and the lifecycle-eof check is guaranteed to wake us:
        //   1. We take buf lock (driver-push blocked).
        //   2. We register waker.
        //   3. We drop buf lock (driver-push may now proceed and fire waker).
        //   4. We take lifecycle lock to check eof.
        //   5. Return Pending or Ready(0); if a push raced through step 3, the waker is registered
        //      and a fresh poll will see the new bytes.
        recv_state.waker.register(cx.waker());
        drop(recv);
        if self.state.lifecycle_lock().recv_eof() {
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

        // Reject writes once the upgrade has begun closing — either the user called
        // `poll_close`, or `submit_trailers` staged trailers + close. Reject as well in
        // terminal states. Anything else (normal upgrade flow) is `UpgradeOpen` and
        // accepts writes.
        // Only `UpgradeOpen` accepts writes. Anything else (upgrade past close, terminal
        // states, or a non-upgrade lifecycle that never had an `H2OutboundReader` to
        // drain) returns `BrokenPipe` — the `AsyncWrite` impl is structurally meaningful
        // only during an active extended-CONNECT upgrade.
        if !matches!(
            &*self.state.lifecycle_lock(),
            StreamLifecycle::UpgradeOpen { .. }
        ) {
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
        // Mark the upgrade write-half closed by transitioning `UpgradeOpen` →
        // `UpgradeClosing`. Once the driver drains the remaining outbound bytes, the
        // upgrade body's `poll_read` will return `Ready(0)`, the send pump transitions
        // through trailers (none) into `DATA(END_STREAM)`, and the stream tears down via
        // the normal `complete_and_remove_stream` path. Idempotent on any non-`UpgradeOpen`
        // state — `poll_close` is allowed to be called multiple times.
        log::trace!(
            "h2 stream {}: H2Transport::poll_close marking outbound closed",
            self.stream_id,
        );
        let mut lifecycle = self.state.lifecycle_lock();
        if let StreamLifecycle::UpgradeOpen { recv_eof } = &*lifecycle {
            *lifecycle = StreamLifecycle::UpgradeClosing {
                recv_eof: *recv_eof,
                pending_trailers: None,
            };
        }
        drop(lifecycle);
        self.state.send.outbound_waker.wake();
        self.connection.outbound_waker().wake();
        Poll::Ready(Ok(()))
    }
}

/// Shared per-stream state. Owned by an [`Arc`] held jointly by the driver (via the connection's
/// stream table) and the handler task (via [`H2Transport`]).
///
/// The cross-task-visible state machine is the [`StreamLifecycle`] held in [`Self::lifecycle`];
/// the recv buffer / wakers / completion signal channel are independent data and stay as
/// sibling fields. See the [`lifecycle`][super::lifecycle] module docs for the rationale.
#[derive(Debug, Default)]
pub(super) struct StreamState {
    /// Cross-task-visible per-stream state machine. The variants encode the legal
    /// observable states of a stream; predicates ([`StreamLifecycle::is_in_flight`],
    /// [`StreamLifecycle::has_pending_recv`], [`StreamLifecycle::recv_eof`],
    /// [`StreamLifecycle::has_active_send`]) and code that needs to make decisions
    /// match on the variants directly.
    pub(super) lifecycle: Mutex<StreamLifecycle>,

    /// Recv side: inbound DATA payloads, handler waker, handler-intent signal, trailers.
    /// Independent of [`Self::lifecycle`] — these are data, not state.
    pub(super) recv: RecvState,

    /// Send side: outbound upgrade buffer + wakers + the driver→conn-task completion
    /// signal channel. Independent of [`Self::lifecycle`].
    pub(super) send: SendState,

    /// Mailbox flag for conn-task → driver work signaling. Set by conn-task code
    /// whenever it produces work the driver should service (lifecycle transition,
    /// [`RecvState::bytes_consumed`] increment, [`RecvState::is_reading`] transition).
    /// The driver's `service_handler_signals` consults this via `swap(false, AcqRel)` —
    /// only streams where it returns `true` pay for the lifecycle-lock pickup. Idle
    /// streams cost a single atomic RMW per tick.
    ///
    /// **Setter ordering rule**: write the underlying state first, then store `true`
    /// with `Release`, then call [`H2Connection::outbound_waker`][super::H2Connection]`.wake()`.
    /// Over-notification is harmless; under-notification would lose a signal.
    pub(super) needs_servicing: AtomicBool,
}

impl StreamState {
    /// Lock the per-stream lifecycle. Convenience wrapper around the inner mutex's
    /// `lock().expect(...)` — every call site treats poisoning as a programming error.
    pub(super) fn lifecycle_lock(&self) -> MutexGuard<'_, StreamLifecycle> {
        self.lifecycle.lock().expect("lifecycle mutex poisoned")
    }
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

    /// Handler-task waker, fired by the driver after pushing DATA into `buf` or after
    /// the lifecycle transitions to recv-eof. Single-waiter: only one task ever polls a
    /// given `H2Transport`.
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
    /// Set to `true` by the driver once the response has been fully framed, flushed, or
    /// errored. The conn task's `SubmitSend` future polls this atomic and registers on
    /// `completion_waker`. Independent signal channel from the lifecycle — on the
    /// extended-CONNECT upgrade path completion is signalled *early* (at `END_HEADERS`,
    /// before the body is on the wire) so the runtime can dispatch `Handler::upgrade`;
    /// the lifecycle stays in `UpgradeOpen` past that point.
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

    /// Waker for the upgrade body's `poll_read`. Fired by [`H2Transport::poll_write`]
    /// after appending bytes and by [`H2Transport::poll_close`] after the lifecycle
    /// transitions to `UpgradeClosing`. Registered by the body during its `poll_read`
    /// when it observes an empty buffer and an `UpgradeOpen` lifecycle.
    pub(super) outbound_waker: AtomicWaker,

    /// Reverse-direction waker: registered by [`H2Transport::poll_write`] when `outbound`
    /// has reached the configured cap, fired by [`H2OutboundReader::poll_read`] after it
    /// drains bytes (i.e. after `ignore_front`) so a parked writer can resume. This is the
    /// edge that surfaces peer flow-control backpressure to the upgrade handler — without
    /// it, a slow or unresponsive peer's closed window would let `outbound` grow without
    /// bound.
    pub(super) outbound_write_waker: AtomicWaker,
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

        // Queue empty. Register first, then re-check the lifecycle. This closes the
        // register-then-check race against `poll_close` / `submit_trailers` (both of
        // which transition the lifecycle but don't take the outbound buf lock).
        send.outbound_waker.register(cx.waker());

        // EOF when the lifecycle has moved past `UpgradeOpen` — `UpgradeClosing`
        // (`poll_close`/`submit_trailers` fired), `Reset*` (peer reset / local error),
        // or terminal. Anything that hasn't reached EOF stays `UpgradeOpen` and we
        // park.
        let lifecycle_says_eof = !matches!(
            &*self.state.lifecycle_lock(),
            StreamLifecycle::UpgradeOpen { .. }
        );
        if lifecycle_says_eof {
            log::trace!(
                "h2 stream {}: H2OutboundReader::poll_read EOF (lifecycle past UpgradeOpen, queue \
                 empty)",
                self.stream_id,
            );
            return Poll::Ready(Ok(0));
        }
        Poll::Pending
    }
}

// `Submission` lives in [`super::lifecycle`] — it's the payload of
// `StreamLifecycle::Submitted`.

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

    /// Build a `(transport, reader)` pair against a fresh `H2Connection`, with the
    /// stream's lifecycle pre-set to `UpgradeOpen` — `H2Transport::poll_write` only
    /// accepts writes for upgrade-lifecycle streams.
    fn pair_with_cap(cap: usize) -> (H2Transport, H2OutboundReader) {
        let mut context = HttpContext::new();
        context.config.response_buffer_max_len = cap;
        let connection = H2Connection::new(Arc::new(context));
        let state = Arc::new(StreamState::default());
        *state.lifecycle_lock() = StreamLifecycle::UpgradeOpen { recv_eof: true };
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

    // Drop / submit_trailers / mark_drop_graceful unit tests are gone — those behaviors
    // are now covered end-to-end by the wire-level fixture in
    // [`super::super::acceptor::tests`] and by the `h2c_*` integration tests in
    // `http/tests/upgrade_matrix.rs`.
}
