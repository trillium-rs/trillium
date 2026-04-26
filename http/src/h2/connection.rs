//! Shared per-connection HTTP/2 state ([`H2Connection`]) plus the [`SubmitSend`] future
//! conn tasks await for response transmission.
//!
//! [`H2Connection`] is `Arc`-shared between the driver task ([`H2Driver`]) and every conn
//! task that holds an open stream's [`Conn`]. It owns the per-stream `StreamState` map,
//! the cross-task wake primitive ([`AtomicWaker`]), and the [`HttpContext`] / [`Swansong`]
//! the broader server stack reaches in through.
//!
//! The driver loop itself lives in [`super::acceptor`] — see that module for the
//! per-connection state machine and how send / receive concerns are split.
//!
//! [`H2Driver`]: super::H2Driver

#[cfg(feature = "unstable")]
use super::H2Initiator;
#[cfg(feature = "unstable")]
use super::transport::H2Transport;
use super::{H2Driver, H2Settings, acceptor::Role, transport::StreamState};
use crate::{Body, Conn, Headers, HttpContext};
use atomic_waker::AtomicWaker;
use event_listener::Event;
#[cfg(feature = "unstable")]
use event_listener::EventListener;
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
    time::{Duration, Instant},
};
use swansong::{ShutdownCompletion, Swansong};

/// Shared per-connection state for HTTP/2.
///
/// Wrapped in an [`Arc`] and held by both the [`H2Driver`] driver and every conn task
/// that holds an open stream's [`Conn`]. Per-stream `StreamState`, HPACK encoder state, and
/// connection-level send flow control will accumulate here as later phases land.
#[derive(Debug)]
pub struct H2Connection {
    context: Arc<HttpContext>,
    swansong: Swansong,
    /// Driver-side waker that conn tasks fire whenever they produce work the driver should
    /// act on — the is-reading signal on first `H2Transport::poll_read`, and the
    /// `submit_send` arrival. Single-consumer (the driver); N producers (conn tasks). The
    /// driver registers its current `drive` waker here each iteration it parks.
    outbound_waker: AtomicWaker,
    /// Per-stream shared state, keyed by stream id. The driver inserts on stream open and
    /// removes on close. Conn-task-side code (`ReceivedBody`, `Conn::send_h2`) looks up
    /// via private accessor methods on `H2Connection` rather than touching the map
    /// directly — `StreamState` stays module-private. The driver also caches each
    /// `Arc<StreamState>` in its private `StreamEntry` for hot-loop perf, so every entry
    /// here has refcount ≥ 2 while the stream is open.
    streams: Mutex<HashMap<u32, Arc<StreamState>>>,
    /// The peer's most recently announced SETTINGS values. Written by the driver each time a
    /// SETTINGS frame arrives (or, for the initial SETTINGS, the first one); read from the
    /// driver's send path when it needs to respect peer-advertised limits (HEADERS fragment
    /// size, stream send-window seed, `MAX_HEADER_LIST_SIZE` cap). Single-task access (only
    /// the driver touches this), so a plain `Mutex` suffices — the `RwLock` optimisation for
    /// concurrent shared reads would be wasted here. `H2Settings` is `Copy`, so readers
    /// typically take the guard, copy out, and release.
    ///
    /// Default-constructed (all fields `None`) means "peer has not yet sent SETTINGS";
    /// readers should use [`H2Settings::effective_*`][H2Settings::effective_max_frame_size]
    /// helpers that apply the RFC 9113 §6.5.2 defaults to absent fields.
    peer_settings: Mutex<H2Settings>,
    /// Latch flipped to `true` the first (and every subsequent) time the driver applies a
    /// peer SETTINGS frame. Distinct from `peer_settings` because an absent field in
    /// `H2Settings` is ambiguous between "peer hasn't sent SETTINGS yet" and "peer sent
    /// SETTINGS without that field" — the latch disambiguates. Read by [`PeerSettings`] to
    /// gate operations that require seeing the peer's first SETTINGS (RFC 8441 §3 extended
    /// CONNECT, in particular).
    peer_settings_received: AtomicBool,
    /// Multi-listener wake source for [`PeerSettings`]. The driver fires `notify(usize::MAX)`
    /// after applying peer SETTINGS and again on connection close, so any number of
    /// concurrently-parked `PeerSettings` futures all unblock together. Using
    /// [`Event`][event_listener::Event] (rather than a single [`AtomicWaker`]) is necessary
    /// because multiple application tasks can call [`H2Connection::peer_settings`]
    /// concurrently — e.g. a fan-out of WebSocket-over-h2 upgrades on one pooled connection
    /// — and an `AtomicWaker`'s last-writer-wins semantics would strand all but one of them.
    peer_settings_event: Event,
    /// Next stream id to allocate for client-role outbound streams. RFC 9113 §5.1.1 requires
    /// client-initiated stream ids to be odd and strictly increasing; we start at 1 and
    /// `+= 2` per allocation. Read/written only by [`Self::open_stream`]; the server role
    /// never touches it. Capped at `2^31` — once exhausted, further `open_stream` calls
    /// return `None` and the caller is expected to fail over to a fresh connection.
    ///
    /// Gated behind `unstable` so server builds (which never call `open_stream`) don't
    /// carry the per-connection allocation. Matches the existing exposure pattern for the
    /// `initiator` module and `H2Connection::run_client`.
    #[cfg(feature = "unstable")]
    next_client_stream_id: Mutex<u32>,
    /// Outstanding active PINGs we've sent and are awaiting ACKs for, keyed by opaque
    /// payload. Populated by [`Self::send_ping`] before the PING is queued for transmission;
    /// completed by the driver when a `PING { ack: true }` arrives whose payload matches an
    /// entry. Drained on connection close so awaiting `send_ping` futures don't leak.
    pending_pings: Mutex<HashMap<[u8; 8], PendingPing>>,
    /// Opaque payloads queued for outbound `PING { ack: false }` emission. The driver
    /// drains this on each [`service_handler_signals`][super::H2Driver] tick. Decoupled
    /// from `pending_pings` so registration and queuing can happen atomically from the
    /// caller's perspective without holding two locks.
    pending_ping_outbound: Mutex<VecDeque<[u8; 8]>>,
}

/// Tracks a single outstanding active PING's lifecycle.
#[derive(Debug)]
pub(crate) struct PendingPing {
    pub(crate) sent_at: Instant,
    pub(crate) waker: Option<Waker>,
    pub(crate) completed: Option<io::Result<Duration>>,
}

/// Future returned by [`H2Connection::send_ping`].
///
/// Resolves to the round-trip time once the peer's PING ACK arrives, or to an `io::Error`
/// if the connection closes first. Dropping the future before completion removes the
/// pending entry so the [`H2Connection`]'s map doesn't accumulate stale state.
#[must_use = "futures do nothing unless awaited"]
#[derive(Debug)]
pub struct SendPing<'a> {
    connection: &'a H2Connection,
    opaque: [u8; 8],
    /// `true` while this future still owns an entry in `pending_pings` that `Drop` must
    /// remove. Set to `false` once registration fails (duplicate opaque) or `poll` returns
    /// `Ready` with the entry removed.
    needs_cleanup: bool,
}

impl Future for SendPing<'_> {
    type Output = io::Result<Duration>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if !this.needs_cleanup {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "PING with this opaque payload is already in flight",
            )));
        }
        let mut pending = this
            .connection
            .pending_pings
            .lock()
            .expect("pending_pings mutex poisoned");
        let entry = pending
            .get_mut(&this.opaque)
            .expect("pending_pings entry removed while SendPing future still pending");
        if let Some(result) = entry.completed.take() {
            pending.remove(&this.opaque);
            this.needs_cleanup = false;
            return Poll::Ready(result);
        }
        entry.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for SendPing<'_> {
    fn drop(&mut self) {
        if self.needs_cleanup
            && let Ok(mut pending) = self.connection.pending_pings.lock()
        {
            pending.remove(&self.opaque);
        }
    }
}

/// Future returned by [`H2Connection::peer_settings`].
///
/// Resolves to `Some(snapshot)` once the driver has applied the peer's first SETTINGS frame,
/// or to `None` if the connection was asked to shut down before any SETTINGS arrived. The
/// `Option` disambiguates "peer never sent SETTINGS" from "peer sent SETTINGS but did not
/// enable the field the caller cares about", which a plain accessor on the snapshot
/// otherwise can't tell apart — both yield `None` for the underlying field.
///
/// The snapshot is a copy of the peer's most recently applied SETTINGS at the moment the
/// future resolves. The peer may send further SETTINGS frames later; for fields where that
/// matters (peer-settable limits like `MAX_CONCURRENT_STREAMS`), follow up with
/// [`H2Connection::peer_settings_snapshot`]. RFC 8441 §3 forbids revoking
/// `SETTINGS_ENABLE_CONNECT_PROTOCOL` once enabled, so a snapshot is sufficient for the
/// extended-CONNECT gate.
///
/// Multiple `PeerSettings` futures can park concurrently on the same connection; all wake
/// together when the driver fires the underlying [`Event`][event_listener::Event].
#[cfg(feature = "unstable")]
#[must_use = "futures do nothing unless awaited"]
#[derive(Debug)]
pub struct PeerSettings<'a>(&'a H2Connection, Option<EventListener>);

#[cfg(feature = "unstable")]
impl Future for PeerSettings<'_> {
    type Output = Option<H2Settings>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self(connection, listener) = &mut *self;
        loop {
            if let Some(snapshot) = connection.peer_settings_snapshot() {
                return Poll::Ready(Some(snapshot));
            }
            if !connection.swansong.state().is_running() {
                return Poll::Ready(None);
            }
            let l = if let Some(l) = listener {
                l
            } else {
                let l = listener.insert(connection.peer_settings_event.listen());
                // Re-check after registering — same load/register/recheck idiom — so a notify
                // racing the registration isn't lost.
                if let Some(snapshot) = connection.peer_settings_snapshot() {
                    return Poll::Ready(Some(snapshot));
                }
                if !connection.swansong.state().is_running() {
                    return Poll::Ready(None);
                }
                l
            };
            std::task::ready!(Pin::new(l).poll(cx));
            *listener = None;
        }
    }
}

/// Future returned by [`H2Connection::response_headers`].
///
/// Awaits the peer's first HEADERS frame on a client-initiated stream and yields the decoded
/// [`FieldSection`][crate::headers::hpack::FieldSection]. See
/// [`H2Connection::response_headers`] for error semantics.
#[cfg(feature = "unstable")]
#[must_use = "futures do nothing unless awaited"]
#[derive(Debug)]
pub struct ResponseHeaders<'a> {
    connection: &'a H2Connection,
    stream_id: u32,
}

#[cfg(feature = "unstable")]
impl Future for ResponseHeaders<'_> {
    type Output = io::Result<crate::headers::hpack::FieldSection<'static>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Some(state) = self.connection.streams_lock().get(&self.stream_id).cloned() else {
            return Poll::Ready(Err(io::ErrorKind::NotConnected.into()));
        };
        let try_take = || {
            state
                .recv
                .response_headers
                .lock()
                .expect("response_headers mutex poisoned")
                .take()
        };
        if let Some(fs) = try_take() {
            return Poll::Ready(Ok(fs));
        }
        if state.recv.eof.load(Ordering::Acquire) {
            return Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()));
        }
        state.recv.response_headers_waker.register(cx.waker());
        // Re-check after registering so we don't miss a wake fired between the load above
        // and the registration.
        if let Some(fs) = try_take() {
            return Poll::Ready(Ok(fs));
        }
        if state.recv.eof.load(Ordering::Acquire) {
            return Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()));
        }
        Poll::Pending
    }
}

impl H2Connection {
    /// Construct a new `H2Connection` to manage HTTP/2 for a single peer.
    pub fn new(context: Arc<HttpContext>) -> Arc<Self> {
        let swansong = context.swansong().child();
        Arc::new(Self {
            context,
            swansong,
            outbound_waker: AtomicWaker::new(),
            streams: Mutex::new(HashMap::new()),
            peer_settings: Mutex::new(H2Settings::default()),
            peer_settings_received: AtomicBool::new(false),
            peer_settings_event: Event::new(),
            #[cfg(feature = "unstable")]
            next_client_stream_id: Mutex::new(1),
            pending_pings: Mutex::new(HashMap::new()),
            pending_ping_outbound: Mutex::new(VecDeque::new()),
        })
    }

    /// The [`HttpContext`] this connection was constructed with.
    pub fn context(&self) -> Arc<HttpContext> {
        self.context.clone()
    }

    /// The connection-scoped [`Swansong`]. Shuts down on peer GOAWAY or when the server-
    /// level swansong shuts down.
    pub fn swansong(&self) -> &Swansong {
        &self.swansong
    }

    /// Attempt graceful shutdown of this HTTP/2 connection.
    pub fn shut_down(&self) -> ShutdownCompletion {
        self.swansong.shut_down()
    }

    /// Send a `PING` frame to the peer and resolve when its `PING ACK` arrives, returning
    /// the round-trip time.
    ///
    /// `opaque` is the 8-byte payload echoed back by the peer (RFC 9113 §6.7). Caller picks
    /// the value — typically a counter or a random nonce. A `PING` whose opaque payload is
    /// already in flight on this connection resolves to `io::ErrorKind::AlreadyExists`.
    ///
    /// No internal timeout. Wrap the returned future with the runtime's
    /// `race_with_timeout` (or equivalent) to bound the wait.
    ///
    /// # Cancel safety
    ///
    /// Dropping the returned future before completion removes the pending entry from this
    /// connection's tracking map. The PING frame may still go out (or already have gone
    /// out) and the peer's ACK is silently dropped. Re-using the same `opaque` after drop
    /// is safe.
    ///
    /// # Panics
    ///
    /// Panics if any of the per-connection mutexes is poisoned (a previous thread panicked
    /// while holding the lock) — same posture as the rest of the h2 driver's mutex usage.
    pub fn send_ping(&self, opaque: [u8; 8]) -> SendPing<'_> {
        let mut pending = self
            .pending_pings
            .lock()
            .expect("pending_pings mutex poisoned");
        if pending.contains_key(&opaque) {
            return SendPing {
                connection: self,
                opaque,
                needs_cleanup: false,
            };
        }
        pending.insert(
            opaque,
            PendingPing {
                sent_at: Instant::now(),
                waker: None,
                completed: None,
            },
        );
        drop(pending);
        self.pending_ping_outbound
            .lock()
            .expect("pending_ping_outbound mutex poisoned")
            .push_back(opaque);
        self.outbound_waker.wake();
        SendPing {
            connection: self,
            opaque,
            needs_cleanup: true,
        }
    }

    /// Driver-side: drain the queue of outbound active PING opaque payloads. Called from
    /// the driver's `service_handler_signals` tick.
    pub(super) fn drain_pending_ping_outbound(&self) -> Vec<[u8; 8]> {
        let mut queue = self
            .pending_ping_outbound
            .lock()
            .expect("pending_ping_outbound mutex poisoned");
        queue.drain(..).collect()
    }

    /// Driver-side: a `PING ACK` for the given opaque payload arrived. Marks the pending
    /// entry complete with the elapsed RTT and wakes its waker, if any. A no-op if the
    /// payload doesn't match an outstanding PING (unsolicited ACK).
    pub(super) fn complete_pending_ping(&self, opaque: [u8; 8]) {
        let mut pending = self
            .pending_pings
            .lock()
            .expect("pending_pings mutex poisoned");
        if let Some(entry) = pending.get_mut(&opaque) {
            let elapsed = entry.sent_at.elapsed();
            entry.completed = Some(Ok(elapsed));
            if let Some(waker) = entry.waker.take() {
                waker.wake();
            }
        }
    }

    /// Driver-side: connection is closing. Complete every outstanding PING with the given
    /// error so awaiting `send_ping` futures don't block forever.
    pub(super) fn fail_pending_pings(&self, error_kind: io::ErrorKind, message: &'static str) {
        let mut pending = self
            .pending_pings
            .lock()
            .expect("pending_pings mutex poisoned");
        for entry in pending.values_mut() {
            if entry.completed.is_none() {
                entry.completed = Some(Err(io::Error::new(error_kind, message)));
                if let Some(waker) = entry.waker.take() {
                    waker.wake();
                }
            }
        }
    }

    /// Driver-side wake primitive. Conn-task code calls
    /// `connection.outbound_waker().wake()` after producing work the driver should service
    /// (an `is_reading` signal, a `submit_send` slot fill).
    pub(super) fn outbound_waker(&self) -> &AtomicWaker {
        &self.outbound_waker
    }

    /// Lock the per-stream `StreamState` map. Used by the driver (insert at stream open,
    /// remove at close) and by conn-task lookups (e.g. `submit_send`).
    pub(super) fn streams_lock(&self) -> MutexGuard<'_, HashMap<u32, Arc<StreamState>>> {
        self.streams
            .lock()
            .expect("connection streams mutex poisoned")
    }

    /// Lock the peer's SETTINGS. Cheap; held only as long as the returned guard lives.
    /// Use the `effective_*` helpers on [`H2Settings`] to get a value with RFC defaults
    /// applied for fields the peer hasn't set; typical callers copy out via `*guard` and
    /// release immediately.
    pub(super) fn current_peer_settings(&self) -> MutexGuard<'_, H2Settings> {
        self.peer_settings
            .lock()
            .expect("peer_settings mutex poisoned")
    }

    /// Whether a fresh stream could be opened on this connection right now.
    ///
    /// Encapsulates the policy a client multiplexer asks before reusing a pooled
    /// connection: the connection must be running (no GOAWAY received, swansong not asked
    /// to shut down) and inflight streams must be below the peer's advertised
    /// `MAX_CONCURRENT_STREAMS`. Future signals (priority pressure under RFC 9218,
    /// flow-control headroom, etc.) can fold into this without changing the call site.
    ///
    /// `false` doesn't mean the connection is dead — it might just be saturated and free
    /// up momentarily. Callers should keep saturated connections in their pool rather than
    /// evicting; pair this with a separate aliveness check to decide eviction.
    ///
    /// # Panics
    ///
    /// Panics if any of the per-connection mutexes is poisoned.
    #[cfg(feature = "unstable")]
    pub fn can_open_stream(&self) -> bool {
        use std::sync::atomic::Ordering;
        if !self.swansong.state().is_running() {
            return false;
        }
        // Count wire-active streams only — entries the application is still holding after a
        // clean wire-close (the h1/h3-symmetric "stream lives until the user drops" lifecycle)
        // are in the map but no longer count against the peer's MAX_CONCURRENT_STREAMS per RFC
        // 9113 §5.1's closed-state rule.
        let inflight: u32 = self
            .streams_lock()
            .values()
            .filter(|s| {
                !(s.send.completed.load(Ordering::Acquire) && s.recv.eof.load(Ordering::Acquire))
            })
            .count()
            .try_into()
            .unwrap_or(u32::MAX);
        let cap = self
            .current_peer_settings()
            .effective_max_concurrent_streams();
        inflight < cap
    }

    /// Park until the driver has applied the peer's first SETTINGS frame.
    ///
    /// The returned [`PeerSettings`] future resolves to `Some(snapshot)` once a peer
    /// SETTINGS frame has been applied at least once, or to `None` if the connection was
    /// asked to shut down before any SETTINGS arrived. On a pooled connection that has
    /// already exchanged SETTINGS, the future resolves on the first poll. Only fresh,
    /// just-handshaked connections actually park.
    ///
    /// Required for callers that send extended-CONNECT requests (RFC 8441 §3 — WebSocket-
    /// over-h2): the spec forbids sending a `:protocol` pseudo-header until the peer has
    /// advertised `SETTINGS_ENABLE_CONNECT_PROTOCOL`. Awaiting this future and then
    /// inspecting the returned [`H2Settings`] snapshot resolves the "peer hasn't sent
    /// SETTINGS yet" vs "peer sent SETTINGS without the field" ambiguity in a single step:
    ///
    /// ```ignore
    /// let Some(settings) = h2.peer_settings().await else {
    ///     // connection shut down before SETTINGS arrived
    /// };
    /// if settings.enable_connect_protocol() != Some(true) {
    ///     // peer doesn't support extended CONNECT
    /// }
    /// ```
    ///
    /// Multiple awaiters on the same connection are supported — internally backed by an
    /// [`Event`][event_listener::Event] rather than a single waker.
    #[cfg(feature = "unstable")]
    pub fn peer_settings(&self) -> PeerSettings<'_> {
        PeerSettings(self, None)
    }

    /// A snapshot of the peer's most recently applied SETTINGS, or `None` if the peer hasn't
    /// sent any SETTINGS frame yet on this connection. The returned [`H2Settings`] is a
    /// `Copy` value owned by the caller; subsequent peer SETTINGS frames will not be
    /// reflected. For a synchronization primitive that parks until the first frame arrives,
    /// see [`Self::peer_settings`].
    ///
    /// Acquire-loaded so the SETTINGS values themselves — written under the
    /// `peer_settings` mutex in [`H2Driver::apply_peer_settings`] — are visible to any
    /// reader who observes the latch as `true`.
    #[cfg(feature = "unstable")]
    pub fn peer_settings_snapshot(&self) -> Option<H2Settings> {
        self.peer_settings_received
            .load(Ordering::Acquire)
            .then(|| *self.current_peer_settings())
    }

    /// Driver-side: a peer SETTINGS frame has just been applied. Latches the
    /// `peer_settings_received` flag and wakes every parked [`PeerSettings`] future.
    /// Idempotent — calling more than once on the same connection is harmless; spurious
    /// wakes are absorbed by the future's poll loop.
    pub(super) fn note_peer_settings(&self) {
        self.peer_settings_received.store(true, Ordering::Release);
        self.peer_settings_event.notify(usize::MAX);
    }

    /// Driver-side: the connection is closing. Wakes every parked [`PeerSettings`] future so
    /// callers awaiting the peer's first SETTINGS observe the shutdown rather than
    /// blocking forever.
    pub(super) fn wake_peer_settings_waiters(&self) {
        self.peer_settings_event.notify(usize::MAX);
    }

    /// Client-role: await the response HEADERS field section for a stream.
    ///
    /// Resolves to the decoded [`FieldSection`] (including h2 pseudo-headers like `:status`)
    /// once the driver receives and stashes the peer's first HEADERS frame on this stream.
    /// Callers typically split pseudos out via [`FieldSection::pseudo_headers`] /
    /// [`into_headers`][FieldSection::into_headers] before populating user-facing
    /// `Headers` + status.
    ///
    /// Single-shot: the `FieldSection` is moved out on a successful poll, so subsequent calls
    /// for the same stream id will surface `ConnectionAborted` rather than re-deliver the
    /// headers.
    ///
    /// Errors:
    /// - `NotConnected` — stream id is no longer tracked by the driver.
    /// - `ConnectionAborted` — recv side reached eof without HEADERS arriving (peer reset the
    ///   stream, sent GOAWAY, or otherwise tore the connection down).
    ///
    /// [`FieldSection`]: crate::headers::hpack::FieldSection
    /// [`FieldSection::pseudo_headers`]: crate::headers::hpack::FieldSection::pseudo_headers
    /// [`FieldSection::into_headers`]: crate::headers::hpack::FieldSection::into_headers
    #[cfg(feature = "unstable")]
    pub fn response_headers(&self, stream_id: u32) -> ResponseHeaders<'_> {
        ResponseHeaders {
            connection: self,
            stream_id,
        }
    }

    /// Remove and return trailers stashed on the stream's recv state. Called by
    /// [`ReceivedBody`][crate::ReceivedBody]'s End transition after the request body is
    /// fully drained. Returns `None` if the stream is gone (already closed) or no trailers
    /// were received.
    pub(crate) fn take_trailers(&self, stream_id: u32) -> Option<Headers> {
        let stream = self.streams_lock().get(&stream_id).cloned()?;
        stream
            .recv
            .trailers
            .lock()
            .expect("recv trailers mutex poisoned")
            .take()
    }

    /// Client-role: signal that the application has dropped its [`H2Transport`] for a
    /// cleanly wire-closed stream and the driver should now remove the entry from both
    /// stream maps. No `RST_STREAM` is emitted — the wire side already closed cleanly via
    /// `END_STREAM` in both directions. This is purely the application-side resource cleanup
    /// trigger (mirroring h1/h3, where the stream lives until the user drops their handle).
    ///
    /// Side effects: sets `StreamState.pending_release` and wakes the driver. No-op on a
    /// stream that's already gone from the map. Server-role streams never reach here —
    /// they're removed eagerly when the response finishes sending.
    pub(crate) fn release_stream(&self, stream_id: u32) {
        let stream = self.streams_lock().get(&stream_id).cloned();
        if let Some(stream) = stream {
            stream.pending_release.store(true, Ordering::Release);
            stream.needs_servicing.store(true, Ordering::Release);
            self.outbound_waker.wake();
        }
    }

    /// Request that the driver emit `RST_STREAM` on this stream with the given error code
    /// and clean up. Called from the conn-task side when something in its path (e.g. a
    /// body-read that detected a content-length violation — RFC 9113 §8.1.2.6) needs the
    /// stream torn down but can't touch the driver's private state directly.
    ///
    /// Side effects: stashes the code on `StreamState.pending_reset` and wakes the driver.
    /// A no-op if the stream is already gone from the shared map — that happens when the
    /// driver has already closed the stream for its own reasons. Idempotent; only the first
    /// call takes effect, subsequent calls see the slot still filled and do nothing.
    pub(crate) fn stream_error(&self, stream_id: u32, code: super::H2ErrorCode) {
        let Some(stream) = self.streams_lock().get(&stream_id).cloned() else {
            return;
        };
        let mut slot = stream
            .pending_reset
            .lock()
            .expect("pending_reset mutex poisoned");
        if slot.is_none() {
            *slot = Some(code);
            drop(slot);
            stream.needs_servicing.store(true, Ordering::Release);
            self.outbound_waker.wake();
        }
    }

    /// Bind this `H2Connection` to a TCP transport and return an [`H2Driver`] that drives
    /// the connection.
    ///
    /// The driver must be polled to completion via repeated calls to
    /// [`H2Driver::next`] (or its [`Stream`][futures_lite::stream::Stream] impl); each returned
    /// [`Conn`] should be spawned on its own task.
    pub fn run<T>(self: Arc<Self>, transport: T) -> H2Driver<T>
    where
        T: AsyncRead + AsyncWrite + Unpin + Send,
    {
        H2Driver::new(self, transport, Role::Server)
    }

    /// Bind this `H2Connection` to an outbound transport and return an [`H2Initiator`] —
    /// the background-task future a client spawns to drive the connection.
    ///
    /// On first poll the driver writes the 24-byte RFC 9113 §3.4 client preface and its
    /// initial SETTINGS; thereafter it demuxes inbound frames (peer SETTINGS, response
    /// HEADERS / DATA on our streams, etc.) and pumps outbound bytes (new stream opens,
    /// DATA, `WINDOW_UPDATEs`) until the connection closes or errors out.
    ///
    /// Awaiting the returned future resolves with `Ok(())` on graceful close or
    /// `Err(H2Error)` on protocol / I/O failure. Streams are not opened via the future
    /// itself — client code calls stream-open primitives on `H2Connection` (introduced
    /// in a later phase); this future just runs the framing loop.
    #[cfg(feature = "unstable")]
    pub fn run_client<T>(self: Arc<Self>, transport: T) -> H2Initiator<T>
    where
        T: AsyncRead + AsyncWrite + Unpin + Send,
    {
        H2Initiator::new(H2Driver::new(self, transport, Role::Client))
    }

    /// Per-stream entry point — call from the runtime adapter's spawned task for each
    /// [`Conn`] returned by [`H2Driver::next`]. Runs `handler` to produce the response,
    /// then `send_h2` to hand the framed response to the driver.
    ///
    /// Mirrors [`H3Connection::process_inbound_bidi`][crate::h3::H3Connection::process_inbound_bidi]'s
    /// role for h3, except the Conn is already built (the acceptor decoded HEADERS and
    /// validated the request before emitting), so this just runs the handler chain and
    /// sends.
    ///
    /// # Errors
    ///
    /// Returns the [`io::Error`] from `send_h2` if the body's `poll_read` errors or the
    /// underlying transport fails partway through the response.
    pub async fn process_inbound<Transport, Handler, Fut>(
        conn: Conn<Transport>,
        handler: Handler,
    ) -> io::Result<Conn<Transport>>
    where
        Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
        Handler: FnOnce(Conn<Transport>) -> Fut,
        Fut: Future<Output = Conn<Transport>>,
    {
        handler(conn).await.send_h2().await
    }

    /// Hand a fully-encoded response off to the driver for framing and transmission.
    ///
    /// The conn task pre-encodes the response HEADERS into `encoded_headers` (via the
    /// static-or-literal HPACK encoder — no shared state required), takes the response
    /// body off the [`Conn`], and `await`s the returned future. The driver picks up the
    /// submission on its next `service_handler_signals` tick, frames it, and signals
    /// completion.
    ///
    /// Trailers are not a separate argument: the driver pulls them off the body via
    /// [`Body::trailers`] once the body is fully drained, mirroring how h1's send path
    /// works.
    pub(crate) fn submit_send(
        &self,
        stream_id: u32,
        encoded_headers: Vec<u8>,
        body: Option<Body>,
    ) -> SubmitSend {
        let stream = self.streams_lock().get(&stream_id).cloned();
        if let Some(state) = &stream {
            *state
                .send
                .submission
                .lock()
                .expect("send submission mutex poisoned") = Some(super::transport::Submission {
                encoded_headers,
                body,
                is_upgrade: false,
            });
            state.needs_servicing.store(true, Ordering::Release);
            self.outbound_waker.wake();
        }
        SubmitSend { stream_id, stream }
    }

    /// Hand a response off for an extended-CONNECT (RFC 8441) upgrade.
    ///
    /// Frames the response HEADERS without `END_STREAM` and signals
    /// [`SubmitSend`] completion the moment the HEADERS frame is on the wire — instead of
    /// after the body finishes, as [`submit_send`][Self::submit_send] does. That early
    /// completion lets [`Conn::send_h2`][crate::Conn::send_h2] return so the runtime
    /// adapter can dispatch [`Handler::upgrade`][trillium::Handler::upgrade] while the
    /// stream stays open as a bidirectional byte channel.
    ///
    /// Internally constructs an [`H2OutboundReader`][super::transport::H2OutboundReader]
    /// over the per-stream outbound queue ([`SendState::outbound`][outbound]) and submits
    /// it as the response body. The upgrade handler appends bytes via
    /// [`H2Transport`][super::H2Transport]'s `AsyncWrite::poll_write`; the driver's send
    /// pump pulls them via the body's `AsyncRead::poll_read` and frames them as DATA
    /// frames bounded by per-stream + connection send windows. When the handler closes
    /// the transport (or drops it), the reader returns `Ready(0)`, the send pump emits
    /// `DATA(END_STREAM)`, and the stream tears down via the normal
    /// `complete_and_remove_stream` path.
    ///
    /// [outbound]: super::transport::SendState::outbound
    pub(crate) fn submit_upgrade(&self, stream_id: u32, encoded_headers: Vec<u8>) -> SubmitSend {
        let stream = self.streams_lock().get(&stream_id).cloned();
        if let Some(state) = &stream {
            let reader = super::transport::H2OutboundReader::new(state.clone(), stream_id);
            let body = Body::new_streaming(reader, None);
            *state
                .send
                .submission
                .lock()
                .expect("send submission mutex poisoned") = Some(super::transport::Submission {
                encoded_headers,
                body: Some(body),
                is_upgrade: true,
            });
            log::trace!("h2 stream {stream_id}: submit_upgrade — submission staged");
            state.needs_servicing.store(true, Ordering::Release);
            self.outbound_waker.wake();
        }
        SubmitSend { stream_id, stream }
    }

    /// Client-role primitive: allocate a fresh outbound stream id, stage a request submission
    /// for the driver, and return the id, a [`SubmitSend`] tracking the request's send half,
    /// and the per-stream [`H2Transport`] for response-body reads.
    ///
    /// `encoded_headers` is the HPACK-encoded HEADERS block (static-or-literal — no shared
    /// dynamic-table state). `body` is the request body, if any; `None` causes the HEADERS
    /// frame to carry `END_STREAM` and no DATA to be emitted.
    ///
    /// Returns `None` when:
    /// - The 2^31 odd-id space is exhausted (caller should fail over to a new connection), or
    /// - The connection is shutting down (we've received GOAWAY or our own swansong has been asked
    ///   to shut down) — opening another stream would just produce a stream the peer has promised
    ///   to ignore.
    ///
    /// Staging is synchronous and infallible past the `None` checks: the submission is
    /// published via the per-stream [`SendState::submission`][submission] slot and the driver
    /// is woken via [`outbound_waker`][outbound_waker]. The driver's pickup pass observes the
    /// new id in the shared streams map, allocates per-stream flow-control state, and the
    /// existing send pump frames HEADERS + DATA + optional trailing HEADERS as if the
    /// submission had come from the server-side path.
    ///
    /// The returned [`SubmitSend`] resolves once the request has been fully framed and
    /// flushed, or with the relevant `io::Error` on failure. The response side is awaited
    /// separately via [`response_headers`][Self::response_headers] for the response HEADERS,
    /// and the [`H2Transport`]'s `AsyncRead` impl for the response body.
    ///
    /// **`SubmitSend` is drop-safe.** The body, once handed off here, is owned by the
    /// driver's per-stream `SendState`; the driver continues to drain it, frame DATA, emit
    /// trailers / `END_STREAM`, and tear the stream down regardless of whether the caller
    /// awaits or drops the returned `SubmitSend`. Clients that only care about the response
    /// (the typical case) may drop it without polling.
    ///
    /// # Panics
    ///
    /// Panics if any of the per-connection / per-stream mutexes is poisoned (a previous
    /// thread panicked while holding the lock) — same posture as the rest of the h2
    /// driver's mutex usage.
    ///
    /// [submission]: super::transport::SendState::submission
    /// [outbound_waker]: Self::outbound_waker
    #[cfg(feature = "unstable")]
    pub fn open_stream(
        self: &Arc<Self>,
        encoded_headers: Vec<u8>,
        body: Option<Body>,
    ) -> Option<(u32, SubmitSend, H2Transport)> {
        self.open_stream_inner(encoded_headers, body, false)
            .map(|(id, state, transport)| {
                (
                    id,
                    SubmitSend {
                        stream_id: id,
                        stream: Some(state),
                    },
                    transport,
                )
            })
    }

    /// Client-role: open a stream for an extended-CONNECT bootstrap (RFC 8441 §3 — WebSocket-
    /// over-h2; the in-progress `draft-ietf-webtrans-http2` for WebTransport-over-h2).
    ///
    /// `encoded_headers` is the HPACK-encoded HEADERS block; the caller is responsible for
    /// ensuring it carries `:method = CONNECT` and a `:protocol` pseudo-header. This is the
    /// only case where staging a stream without a request body is *not* terminated by
    /// `END_STREAM` on the initial HEADERS — instead, the per-stream outbound queue (the same
    /// one [`H2Transport`]'s `AsyncWrite::poll_write` appends to) becomes the request body
    /// and stays open until the application closes the transport.
    ///
    /// Returns `(stream_id, H2Transport)` — no [`SubmitSend`]. The application reads response
    /// HEADERS via [`Self::response_headers`] and then exchanges bytes over the returned
    /// transport's `AsyncRead` + `AsyncWrite`.
    ///
    /// Returns `None` under the same conditions as [`Self::open_stream`]: stream-id space
    /// exhausted, or connection shutting down.
    ///
    /// **Caller MUST first await
    /// [`peer_settings`][Self::peer_settings] and verify the
    /// returned snapshot's `enable_connect_protocol() == Some(true)` before calling this.**
    /// Sending extended CONNECT to a peer that hasn't advertised
    /// `SETTINGS_ENABLE_CONNECT_PROTOCOL = 1` is a protocol violation per RFC 8441 §3.
    #[cfg(feature = "unstable")]
    pub fn open_connect_stream(
        self: &Arc<Self>,
        encoded_headers: Vec<u8>,
    ) -> Option<(u32, H2Transport)> {
        let (id, _state, transport) = self.open_stream_inner(encoded_headers, None, true)?;
        Some((id, transport))
    }

    /// Shared id-allocate-and-stage logic backing [`Self::open_stream`] and
    /// [`Self::open_connect_stream`]. The `is_upgrade` flag controls two things in the driver's
    /// send pump: HEADERS does not carry `END_STREAM` (because the body field is `Some`), and
    /// the body is sourced from the per-stream outbound queue ([`H2OutboundReader`]) rather
    /// than the caller-supplied `Body`. For the non-upgrade path, the caller-supplied `body`
    /// is used as-is and `END_STREAM` semantics fall out of `body.is_none()`.
    #[cfg(feature = "unstable")]
    fn open_stream_inner(
        self: &Arc<Self>,
        encoded_headers: Vec<u8>,
        body: Option<Body>,
        is_upgrade: bool,
    ) -> Option<(u32, Arc<StreamState>, H2Transport)> {
        if !self.swansong.state().is_running() {
            return None;
        }

        let stream_id = {
            let mut next = self
                .next_client_stream_id
                .lock()
                .expect("next_client_stream_id mutex poisoned");
            if *next >= (1u32 << 31) {
                return None;
            }
            let id = *next;
            *next += 2;
            id
        };

        let state = Arc::new(StreamState::default());

        // For an extended-CONNECT bootstrap, the body field of the submission must be the
        // per-stream outbound queue — same shape the server-side `submit_upgrade` uses.
        // That keeps HEADERS flowing without END_STREAM and turns the per-stream
        // outbound buffer into the writeback channel reachable through `H2Transport`'s
        // `AsyncWrite`.
        let body = if is_upgrade {
            let reader = super::transport::H2OutboundReader::new(state.clone(), stream_id);
            Some(Body::new_streaming(reader, None))
        } else {
            body
        };

        // Stage submission *before* publishing the stream id to the shared map. The driver's
        // client-pickup pass scans the shared map, allocates a `StreamEntry`, and on the same
        // tick the existing submission-pickup loop promotes this submission to a `SendCursor`.
        // Doing it in this order means the submission is guaranteed visible the first time
        // the driver sees the stream — no second tick needed to start framing.
        *state
            .send
            .submission
            .lock()
            .expect("send submission mutex poisoned") = Some(super::transport::Submission {
            encoded_headers,
            body,
            is_upgrade,
        });
        state.needs_servicing.store(true, Ordering::Release);
        self.streams_lock().insert(stream_id, state.clone());
        log::trace!("h2 client: open_stream allocated stream {stream_id} (upgrade={is_upgrade})");
        self.outbound_waker.wake();
        let transport = H2Transport::new(Arc::clone(self), stream_id, state.clone());
        Some((stream_id, state, transport))
    }
}

/// Future returned by the various send-staging primitives on [`H2Connection`]; resolves once
/// the driver has fully framed and flushed the submitted message (request on the client,
/// response on the server), or with the relevant `io::Error` on failure.
///
/// Holds the per-stream [`StreamState`] Arc (cloned out of the streams map at submit time),
/// not a connection backref + id — so dropping the future doesn't require another map
/// lookup and the conn task's wake registration stays local to the per-stream sync
/// primitives.
#[must_use = "futures do nothing unless awaited"]
#[derive(Debug)]
pub struct SubmitSend {
    stream_id: u32,
    /// `None` if the stream wasn't in the map at submit time (already closed). The future
    /// surfaces that as `NotConnected`.
    stream: Option<Arc<StreamState>>,
}

impl Future for SubmitSend {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Some(state) = &self.stream else {
            log::debug!("h2 stream {}: submit_send on closed stream", self.stream_id);
            return Poll::Ready(Err(io::ErrorKind::NotConnected.into()));
        };

        let stream_id = self.stream_id;
        let try_take = || -> Option<io::Result<()>> {
            state.send.completed.load(Ordering::Acquire).then(|| {
                state
                    .send
                    .completion_result
                    .lock()
                    .expect("completion_result mutex poisoned")
                    .take()
                    .unwrap_or_else(|| {
                        log::error!(
                            "h2 stream {stream_id}: completed without a completion_result — \
                             driver should write the result before flipping completed"
                        );
                        Ok(())
                    })
            })
        };

        if let Some(result) = try_take() {
            return Poll::Ready(result);
        }
        state.send.completion_waker.register(cx.waker());
        // Re-check after registering so we don't miss a wake fired between the load above
        // and the registration.
        if let Some(result) = try_take() {
            return Poll::Ready(result);
        }
        Poll::Pending
    }
}
