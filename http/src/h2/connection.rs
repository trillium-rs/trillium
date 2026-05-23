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
    /// removes on close. Conn-task code looks up via private accessors on `H2Connection`
    /// rather than touching the map directly — `StreamState` stays module-private.
    pub(super) streams: Mutex<HashMap<u32, Arc<StreamState>>>,
    /// The peer's most recently announced SETTINGS values. The driver writes on every
    /// inbound SETTINGS frame and is the only reader, so a plain `Mutex` suffices.
    /// `H2Settings` is `Copy`, so readers take the guard, copy out, and release.
    ///
    /// Default-constructed (all fields `None`) means "peer has not yet sent SETTINGS";
    /// readers should use [`H2Settings::effective_*`][H2Settings::effective_max_frame_size]
    /// helpers that apply the RFC defaults to absent fields.
    pub(super) peer_settings: Mutex<H2Settings>,
    /// Latch flipped to `true` the first (and every subsequent) time the driver applies
    /// a peer SETTINGS frame. Distinct from `peer_settings` because an absent field is
    /// ambiguous between "peer hasn't sent SETTINGS yet" and "peer sent SETTINGS without
    /// that field" — the latch disambiguates, gating operations that require having seen
    /// the peer's first SETTINGS (e.g. extended CONNECT).
    pub(super) peer_settings_received: AtomicBool,
    /// Multi-listener wake source for [`PeerSettings`]. The driver fires `notify(usize::MAX)`
    /// after applying peer SETTINGS and again on connection close, so any number of
    /// concurrently-parked `PeerSettings` futures all unblock together. [`Event`] (rather
    /// than a single [`AtomicWaker`]) is required because multiple application tasks can
    /// park on `peer_settings` concurrently — e.g. a fan-out of WebSocket-over-h2 upgrades
    /// on one pooled connection — and `AtomicWaker`'s last-writer-wins semantics would
    /// strand all but one.
    pub(super) peer_settings_event: Event,
    /// Next stream id to allocate for client-role outbound streams. Starts at 1 and
    /// `+= 2` per allocation. Capped at `2^31` — once exhausted, `fetch_update`'s closure
    /// refuses to advance, and `open_stream` returns `None` (the caller is expected to
    /// fail over to a fresh connection).
    #[cfg(feature = "unstable")]
    pub(super) next_client_stream_id: std::sync::atomic::AtomicU32,
    /// Outstanding active PINGs awaiting ACKs, keyed by opaque payload. Completed by the
    /// driver when a `PING { ack: true }` arrives whose payload matches an entry. Drained
    /// on connection close so awaiting `send_ping` futures don't leak.
    pub(super) pending_pings: Mutex<HashMap<[u8; 8], PendingPing>>,
    /// Opaque payloads queued for outbound `PING { ack: false }` emission. Decoupled from
    /// `pending_pings` so registration and queuing can happen without holding two locks.
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
    /// `true` requires: the connection is running (no GOAWAY received, swansong not asked
    /// to shut down), inflight streams are below the peer's advertised
    /// `MAX_CONCURRENT_STREAMS`, and the client stream-id space is not exhausted (capped
    /// at `2^31 - 1`).
    ///
    /// `false` doesn't mean the connection is dead — it might just be saturated and free
    /// up momentarily. Callers should keep saturated connections in their pool rather than
    /// evicting; pair this with a separate aliveness check to decide eviction.
    ///
    /// Stream-id exhaustion is the one "false" case that *is* permanent: the connection
    /// will never accept another `open_stream` call, though in-flight streams will still
    /// complete.
    ///
    /// # Panics
    ///
    /// Panics if any per-connection mutex is poisoned.
    #[cfg(feature = "unstable")]
    pub fn can_open_stream(&self) -> bool {
        if !self.swansong.state().is_running() {
            return false;
        }
        // Stream-id exhaustion check guards against an exhausted connection passing the
        // inflight-vs-MAX_CONCURRENT_STREAMS check (no streams in flight → counts as 0)
        // and the pool selecting it as Available, only for `open_stream` to fail at the
        // call site with a misleading "shutting down" error.
        if self.next_client_stream_id.load(Ordering::Relaxed) >= (1u32 << 31) {
            return false;
        }
        // Count wire-active streams only — entries the application is still holding after
        // a clean wire-close stay in the map but don't count against the peer's
        // MAX_CONCURRENT_STREAMS.
        let inflight: u32 = self
            .streams_lock()
            .values()
            .filter(|s| !s.lifecycle_lock().is_wire_closed())
            .count()
            .try_into()
            .unwrap_or(u32::MAX);
        let cap = self
            .current_peer_settings()
            .effective_max_concurrent_streams();
        inflight < cap
    }

    /// Driver-side wake primitive. Fire after producing work the driver should service.
    pub(super) fn outbound_waker(&self) -> &AtomicWaker {
        &self.outbound_waker
    }

    /// Lock the per-stream `StreamState` map.
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

    // `release_stream` (previously a conn-level helper called from `H2Transport::Drop`)
    // is no longer needed — the Drop impl transitions the lifecycle to `AwaitingRelease`
    // directly. The transition has the same observable effects (wake driver, raise
    // `needs_servicing`, mark variant) and removes one indirection.

    /// Request that the driver emit `RST_STREAM` on this stream with the given error code
    /// and clean up. Transitions the lifecycle to
    /// [`StreamLifecycle::ResetRequested`][super::lifecycle::StreamLifecycle::ResetRequested]
    /// and wakes the driver.
    ///
    /// First-wins idempotent: a stream already in `ResetRequested` or terminal `Reset`
    /// state does not have its code overwritten. No-op if the stream is already gone from
    /// the shared map.
    pub(crate) fn stream_error(&self, stream_id: u32, code: super::H2ErrorCode) {
        let Some(stream) = self.streams_lock().get(&stream_id).cloned() else {
            return;
        };
        let mut lifecycle = stream.lifecycle_lock();
        if matches!(
            &*lifecycle,
            super::lifecycle::StreamLifecycle::ResetRequested(_)
                | super::lifecycle::StreamLifecycle::Reset(_)
        ) {
            return;
        }
        *lifecycle = super::lifecycle::StreamLifecycle::ResetRequested(code);
        drop(lifecycle);
        stream.needs_servicing.store(true, Ordering::Release);
        self.outbound_waker.wake();
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
    /// On first poll the driver writes the 24-byte client preface and its initial
    /// SETTINGS; thereafter it demuxes inbound frames (peer SETTINGS, response HEADERS /
    /// DATA on our streams, etc.) and pumps outbound bytes (new stream opens, DATA,
    /// `WINDOW_UPDATEs`) until the connection closes or errors out.
    ///
    /// Awaiting the returned future resolves with `Ok(())` on graceful close or
    /// `Err(H2Error)` on protocol / I/O failure. Streams are not opened via the future
    /// itself — client code calls stream-open primitives on `H2Connection`; this future
    /// just runs the framing loop.
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
        let _guard = conn.context().swansong().guard();
        handler(conn).await.send_h2().await
    }
}
