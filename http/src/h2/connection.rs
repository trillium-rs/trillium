//! Shared per-connection HTTP/2 state ([`H2Connection`]).
//!
//! [`H2Connection`] is `Arc`-shared between the driver task ([`H2Driver`]) and every conn
//! task that holds an open stream's [`Conn`]. It owns the per-stream `StreamState` map,
//! the cross-task wake primitive ([`AtomicWaker`]), and the [`HttpContext`] / [`Swansong`]
//! the broader server stack reaches in through.
//!
//! The driver loop itself lives in [`super::acceptor`] — see that module for the
//! per-connection state machine and how send / receive concerns are split.
//!
//! # Module layout
//!
//! Conn-task-side primitives are split across child modules so each subsystem reads
//! independently:
//!
//! - [`ping`]: `PING` / `PING ACK` round-trip tracking and the [`SendPing`] future.
//! - [`peer_settings_wait`]: the [`PeerSettings`] sync primitive that parks until the peer's first
//!   SETTINGS frame is applied.
//! - [`submit`]: send-staging API ([`submit_send`][H2Connection::submit_send],
//!   [`submit_upgrade`][H2Connection::submit_upgrade]) and client-side stream-open primitives
//!   ([`open_stream`][H2Connection::open_stream] /
//!   [`open_connect_stream`][H2Connection::open_connect_stream]) + the [`SubmitSend`] future.
//! - [`response`]: client-role recv-side primitives — [`ResponseHeaders`] and
//!   [`take_trailers`][H2Connection::take_trailers].
//!
//! [`H2Driver`]: super::H2Driver

mod peer_settings_wait;
mod ping;
mod response;
mod submit;

#[cfg(feature = "unstable")]
use super::H2Initiator;
use super::{H2Driver, H2Settings, role::Role, transport::StreamState};
use crate::{Conn, HttpContext};
use atomic_waker::AtomicWaker;
use event_listener::Event;
use futures_lite::io::{AsyncRead, AsyncWrite};
use ping::PendingPing;
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    io,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
};
use swansong::{ShutdownCompletion, Swansong};
#[cfg(feature = "unstable")]
#[allow(unused_imports)]
// re-exports for h2.rs's `pub use connection::{ResponseHeaders, SubmitSend}`
pub use {response::ResponseHeaders, submit::SubmitSend};

/// Shared per-connection state for HTTP/2.
///
/// Wrapped in an [`Arc`] and held by both the [`H2Driver`] driver and every conn task
/// that holds an open stream's [`Conn`]. Per-stream `StreamState`, HPACK encoder state, and
/// connection-level send flow control lives here.
#[derive(Debug)]
pub struct H2Connection {
    pub(super) context: Arc<HttpContext>,
    pub(super) swansong: Swansong,
    /// Driver-side waker that conn tasks fire whenever they produce work the driver should
    /// act on — the is-reading signal on first `H2Transport::poll_read`, and the
    /// `submit_send` arrival. Single-consumer (the driver); N producers (conn tasks). The
    /// driver registers its current `drive` waker here each iteration it parks.
    pub(super) outbound_waker: AtomicWaker,
    /// Per-stream shared state, keyed by stream id. The driver inserts on stream open and
    /// removes on close. Conn-task-side code (`ReceivedBody`, `Conn::send_h2`) looks up
    /// via private accessor methods on `H2Connection` rather than touching the map
    /// directly — `StreamState` stays module-private. The driver also caches each
    /// `Arc<StreamState>` in its private `StreamEntry` for hot-loop perf, so every entry
    /// here has refcount ≥ 2 while the stream is open.
    pub(super) streams: Mutex<HashMap<u32, Arc<StreamState>>>,
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
    pub(super) peer_settings: Mutex<H2Settings>,
    /// Latch flipped to `true` the first (and every subsequent) time the driver applies a
    /// peer SETTINGS frame. Distinct from `peer_settings` because an absent field in
    /// `H2Settings` is ambiguous between "peer hasn't sent SETTINGS yet" and "peer sent
    /// SETTINGS without that field" — the latch disambiguates. Read by [`PeerSettings`] to
    /// gate operations that require seeing the peer's first SETTINGS (RFC 8441 §3 extended
    /// CONNECT, in particular).
    pub(super) peer_settings_received: AtomicBool,
    /// Multi-listener wake source for [`PeerSettings`]. The driver fires `notify(usize::MAX)`
    /// after applying peer SETTINGS and again on connection close, so any number of
    /// concurrently-parked `PeerSettings` futures all unblock together. Using
    /// [`Event`][event_listener::Event] (rather than a single [`AtomicWaker`]) is necessary
    /// because multiple application tasks can call [`H2Connection::peer_settings`]
    /// concurrently — e.g. a fan-out of WebSocket-over-h2 upgrades on one pooled connection
    /// — and an `AtomicWaker`'s last-writer-wins semantics would strand all but one of them.
    pub(super) peer_settings_event: Event,
    /// Next stream id to allocate for client-role outbound streams. RFC 9113 §5.1.1 requires
    /// client-initiated stream ids to be odd and strictly increasing; we start at 1 and
    /// `+= 2` per allocation via [`AtomicU32::fetch_update`]. Read/written only by
    /// [`Self::open_stream`]; the server role never touches it. Capped at `2^31` — once
    /// exhausted, `fetch_update`'s closure returns `None` so the counter stops advancing
    /// and further `open_stream` calls return `None` (the caller is expected to fail over
    /// to a fresh connection).
    ///
    /// Gated behind `unstable` so server builds (which never call `open_stream`) don't
    /// carry the field at all. Matches the existing exposure pattern for the `initiator`
    /// module and `H2Connection::run_client`.
    #[cfg(feature = "unstable")]
    pub(super) next_client_stream_id: std::sync::atomic::AtomicU32,
    /// Outstanding active PINGs we've sent and are awaiting ACKs for, keyed by opaque
    /// payload. Populated by [`Self::send_ping`] before the PING is queued for transmission;
    /// completed by the driver when a `PING { ack: true }` arrives whose payload matches an
    /// entry. Drained on connection close so awaiting `send_ping` futures don't leak.
    pub(super) pending_pings: Mutex<HashMap<[u8; 8], PendingPing>>,
    /// Opaque payloads queued for outbound `PING { ack: false }` emission. The driver
    /// drains this on each [`service_handler_signals`][super::H2Driver] tick. Decoupled
    /// from `pending_pings` so registration and queuing can happen atomically from the
    /// caller's perspective without holding two locks.
    pub(super) pending_ping_outbound: Mutex<VecDeque<[u8; 8]>>,
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
            next_client_stream_id: std::sync::atomic::AtomicU32::new(1),
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

    /// Whether a fresh stream could be opened on this connection right now.
    ///
    /// Encapsulates the policy a client multiplexer asks before reusing a pooled
    /// connection: the connection must be running (no GOAWAY received, swansong not asked
    /// to shut down), inflight streams must be below the peer's advertised
    /// `MAX_CONCURRENT_STREAMS`, and the client stream-id space must not be exhausted
    /// (RFC 9113 §5.1.1 caps client-initiated stream ids at `2^31 - 1`). Future signals
    /// (priority pressure under RFC 9218, flow-control headroom, etc.) can fold into
    /// this without changing the call site.
    ///
    /// `false` doesn't mean the connection is dead — it might just be saturated and free
    /// up momentarily. Callers should keep saturated connections in their pool rather than
    /// evicting; pair this with a separate aliveness check to decide eviction.
    ///
    /// Stream-id exhaustion is the one "false" case that *is* permanent: the connection
    /// will never accept another `open_stream` call. The caller's pool should treat this
    /// the same as `MAX_CONCURRENT_STREAMS` saturation (Busy → fall through to a fresh
    /// connection); the connection is still usable for in-flight stream completion.
    ///
    /// # Panics
    ///
    /// Panics if any of the per-connection mutexes is poisoned.
    #[cfg(feature = "unstable")]
    pub fn can_open_stream(&self) -> bool {
        if !self.swansong.state().is_running() {
            return false;
        }
        // Stream-id space exhausted: a fresh `open_stream` would return `None` because
        // `fetch_update`'s closure refuses to advance past the cap. Without this check,
        // an exhausted connection passes the inflight-vs-MAX_CONCURRENT_STREAMS check
        // (no streams in flight → counts as 0) and the pool selects it as Available,
        // only for `open_stream` to fail with a misleading "shutting down" error at the
        // call site.
        if self.next_client_stream_id.load(Ordering::Relaxed) >= (1u32 << 31) {
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
}
