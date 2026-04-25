//! HTTP/2 driver loop ([`H2Driver`]) — owns the per-connection TCP transport and runs the
//! poll-based state machine that demuxes frames, dispatches stream-opens to handler tasks, and
//! pumps responses back out.
//!
//! Created by [`H2Connection::run`]. The runtime adapter calls [`H2Driver::next`] in a
//! loop (or drives via the [`Stream`] impl, which has the same semantics); each yield either
//! returns the next opened request stream (a [`Conn`] for the runtime to spawn a handler
//! task against) or `None` when the connection is closed.
//!
//! The driver is a poll-based state machine, not an async fn. A single `drive` call is the
//! unit of forward progress: it picks up conn-task signals, advances any in-flight response
//! sends, drains pending outbound bytes, and advances the read cursor — parking with
//! cancel-safe partial state when no further progress can be made.
//!
//! # Module layout
//!
//! Driver impl is split across this file and two child modules to keep each focused:
//!
//! - **`acceptor.rs`** (this file): struct definition, the [`Self::drive`] orchestration loop,
//!   conn-task signal pickup, write/flush plumbing, and the `queue_*` outbound-frame helpers. Also
//!   the supporting enums ([`DriverState`], [`ReadPhase`], [`CloseOutcome`], [`Action`],
//!   [`StreamEntry`]).
//! - **`acceptor::recv`**: receive side — frame reader, dispatch, HEADERS+CONTINUATION
//!   accumulation, malformed-request `RST_STREAM`, DATA routing into per-stream recv rings.
//! - **`acceptor::send`**: send pump — picks up [`SendCursor`][send::SendCursor]s from the
//!   conn-task signal pickup, frames HEADERS / DATA / trailing-HEADERS, signals completion.
//!
//! [`H2Connection::run`]: super::H2Connection::run
//! [`Stream`]: futures_lite::stream::Stream

mod recv;
mod send;

use super::{
    H2Error, H2ErrorCode, H2Settings,
    connection::H2Connection,
    frame::{self, FRAME_HEADER_LEN},
    transport::{H2Transport, StreamState},
};
use crate::{Conn, headers::hpack::HpackDecoder};
use futures_lite::io::{AsyncRead, AsyncWrite};
use recv::PendingHeaders;
use send::SendCursor;
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, atomic::Ordering},
    task::{Context, Poll, ready},
};
use swansong::ShuttingDown;

/// Absolute upper bound on transient frame buffers — a backstop against a peer that advertises
/// an absurd frame size. Independent of `HttpConfig::h2_max_frame_size` (which we advertise and
/// enforce against incoming frames); this is just the ceiling on our own decode buffer to
/// prevent runaway allocation under an adversarial peer.
const MAX_BUFFER_SIZE: usize = 1 << 20;

/// Initial HPACK dynamic table size per RFC 7541 §4.2 — also the value implied by an absent
/// `SETTINGS_HEADER_TABLE_SIZE`. HPACK dynamic table is decode-only today (encoder is
/// static-or-literal), so a user-facing knob here would be cosmetic. Revisit when dynamic
/// encoding lands.
const HPACK_TABLE_SIZE: usize = 4096;

/// RFC 9113 §6.9.2 baseline connection-level flow-control window — 65535 octets for both
/// directions, unchanged by SETTINGS. Used as the starting value for our send-side window
/// (credited via peer `WINDOW_UPDATE(0)`) and for our recv-side window before we emit the
/// initial raising `WINDOW_UPDATE(0)` to `h2_initial_connection_window_size`.
const INITIAL_CONNECTION_RECV_WINDOW: i64 = 65_535;

/// Hard ceiling on the DATA payload we'll emit in a single frame even if the peer
/// advertises a larger `MAX_FRAME_SIZE`. Bounds `body_scratch` so a permissive peer can't
/// steer us into oversized allocations; the protocol only requires we not *exceed* the
/// peer's advertised max, which starts at the RFC 9113 §6.5.2 default of 16 KiB.
const MAX_DATA_CHUNK_SIZE: u32 = 16_384;

/// RFC 9113 §6.9.1: a flow-control window MUST NOT exceed `2^31 - 1`. If a
/// `WINDOW_UPDATE` would push it past that maximum, the peer has misbehaved — we emit
/// `FLOW_CONTROL_ERROR` at the appropriate level (connection or stream).
pub(super) const MAX_FLOW_CONTROL_WINDOW: i64 = (1 << 31) - 1;

/// Whether this driver is servicing a peer that dialled us (server role) or a peer we
/// dialled (client role). Routes the handful of role-asymmetric driver concerns — preface
/// direction, HEADERS-on-unknown-id semantics, HEADERS-on-known-id semantics — through a
/// single match point each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    /// Driver was handed a transport from an accepting listener — we read the client
    /// preface, treat peer-initiated (odd-id) streams as new requests, and treat HEADERS
    /// on a known stream as trailers.
    Server,
    /// Driver was handed a transport from an outbound dial — we write the client preface,
    /// open streams with locally-allocated odd ids, and treat HEADERS on one of our
    /// streams as the response headers (first arrival) or trailers (second). Produced by
    /// [`H2Connection::run_client`][super::H2Connection::run_client], which is gated
    /// behind the `unstable` feature — without that feature the variant is defined but
    /// never constructed.
    #[cfg_attr(not(feature = "unstable"), allow(dead_code))]
    Client,
}

/// Owns the per-connection TCP transport and drives the HTTP/2 demux loop.
///
/// See the [module docs](self) for the high-level driver shape and how its impl is split
/// across the `recv` and `send` child modules.
#[derive(Debug)]
pub struct H2Driver<T> {
    connection: Arc<H2Connection>,
    transport: T,

    /// Role this driver runs in — see [`Role`]. Consulted at role-asymmetric branch points
    /// (preface direction, HEADERS-on-unknown-id, HEADERS-on-known-id).
    role: Role,

    /// Overall lifecycle position of the driver.
    state: DriverState,

    /// Future that resolves when the shared `Swansong` begins shutdown. Polled each
    /// `drive` tick while the driver is running; on resolution the driver queues a
    /// GOAWAY and transitions to `Closing`, after which the top-of-loop guard returns
    /// early and we never poll this again on the same acceptor.
    shutting_down: ShuttingDown,

    /// Inbound byte cursor. Accumulates bytes from the transport across `drive` calls so
    /// a partial frame read can survive a return to `Poll::Pending`. Always contains
    /// exactly the bytes of the current frame being accumulated (header, then payload);
    /// reset after each complete frame is dispatched.
    read_buf: Vec<u8>,
    read_filled: usize,
    read_phase: ReadPhase,

    /// Outbound byte cursor. The driver encodes control frames into `write_buf` and drains
    /// to the transport via `poll_flush_outbound`. `write_cursor` is the offset of the
    /// first byte not yet accepted by `poll_write`. After the buffer fully drains, both
    /// fields are reset and a flush is issued.
    write_buf: Vec<u8>,
    write_cursor: usize,
    write_flush_pending: bool,

    /// HPACK decoder state, shared across all header blocks on this connection.
    hpack: HpackDecoder,

    /// Per-stream state, keyed by stream id. Driver-only — handler tasks hold their own
    /// `Arc<StreamState>` via [`H2Transport`] and don't consult this table. The entry
    /// bundles the shared state with driver-private bookkeeping (e.g. "have we already
    /// advertised the recv window after seeing `is_reading`?").
    streams: HashMap<u32, StreamEntry>,

    /// Highest peer-initiated stream id seen so far. Peer-initiated (client) stream ids
    /// must be odd and strictly increasing per RFC 9113 §5.1.1.
    last_peer_stream_id: u32,

    /// Accumulator for an in-progress HEADERS block that is waiting on further CONTINUATION
    /// frames. `None` outside a HEADERS block. §6.10 forbids any frame on any stream from
    /// interleaving while this is `Some`.
    pending_headers: Option<PendingHeaders>,

    /// Set once the driver decides to close: graceful (peer GOAWAY / server swansong / peer
    /// EOF) or erroring (protocol violation → GOAWAY with code, or I/O failure → no
    /// GOAWAY). `drive` completes (returns `None` or a final `Some(Err(...))`) once
    /// outbound drains to empty.
    close_outcome: Option<CloseOutcome>,

    /// Set after `drive` yields its terminal result. Subsequent calls return `None` without
    /// touching the transport.
    finished: bool,

    /// Reusable scratch the send pump reads body chunks into before framing as DATA.
    /// Sized at [`MAX_DATA_CHUNK_SIZE`] — even if the peer permits larger frames we cap our
    /// DATA emissions here to bound per-connection memory.
    body_scratch: Vec<u8>,

    /// Connection-level send flow-control window (RFC 9113 §6.9). Tracked as [`i64`] so
    /// mid-connection `INITIAL_WINDOW_SIZE` reductions can drive per-stream windows
    /// temporarily negative (§6.9.2) — kept here to the connection window for symmetry
    /// though the connection window itself is *not* affected by `SETTINGS_INITIAL_WINDOW_SIZE`.
    /// Decremented as we emit DATA; incremented by peer `WINDOW_UPDATE(stream_id=0, inc)`.
    /// Overflow past [`MAX_FLOW_CONTROL_WINDOW`] is a connection-level `FLOW_CONTROL_ERROR`.
    connection_send_window: i64,

    /// Connection-level recv flow-control window. Starts at the RFC 9113 §6.9.2 baseline of
    /// 65535 octets and is raised to [`MAX_CONNECTION_RECV_WINDOW`] via an initial
    /// `WINDOW_UPDATE(0)` right after SETTINGS — §6.9.2 forbids SETTINGS from altering it,
    /// so WU is the only path. Decremented as peer DATA frames arrive (across all streams);
    /// incremented as the handler-task-side consumption signal is picked up and we emit
    /// `WINDOW_UPDATE(0, consumed)`. A negative value means the peer overran the window —
    /// connection-level `FLOW_CONTROL_ERROR`.
    connection_recv_window: i64,

    /// Bounded ledger of recently-closed streams and why they closed. Consulted by
    /// [`recv::H2Driver::finalize_headers`] when a HEADERS frame arrives on an id ≤
    /// `last_peer_stream_id` that's not in the active map, to distinguish `RST_STREAM`-
    /// closed (stream-level `STREAM_CLOSED`) from `END_STREAM`-closed or never-opened
    /// (connection-level). See [`ClosedStreams`] for the eviction policy.
    closed_streams: ClosedStreams,

    /// Snapshot of the h2-relevant fields of [`HttpConfig`][crate::HttpConfig] taken at
    /// acceptor construction. Copied in because `HttpConfig` is per-server but an acceptor
    /// is per-connection — the config is effectively immutable over a connection's
    /// lifetime, and a local copy avoids reaching through [`H2Connection::context`] on
    /// every policy check.
    ///
    /// [`H2Connection::context`]: super::H2Connection::context
    pub(super) config: AcceptorConfig,
}

/// h2-relevant configuration extracted from [`HttpConfig`][crate::HttpConfig] at acceptor
/// construction. Carried as a plain value so hot-loop policy checks don't cross the
/// `Arc<HttpContext>` indirection.
#[derive(Debug, Clone, Copy)]
pub(super) struct AcceptorConfig {
    pub(super) initial_stream_window_size: u32,
    pub(super) max_stream_recv_window_size: u32,
    pub(super) initial_connection_window_size: u32,
    pub(super) max_concurrent_streams: u32,
    pub(super) max_frame_size: u32,
}

impl AcceptorConfig {
    fn from_http_config(config: &crate::HttpConfig) -> Self {
        Self {
            initial_stream_window_size: config.h2_initial_stream_window_size(),
            max_stream_recv_window_size: config.h2_max_stream_recv_window_size(),
            initial_connection_window_size: config.h2_initial_connection_window_size(),
            max_concurrent_streams: config.h2_max_concurrent_streams(),
            max_frame_size: config.h2_max_frame_size(),
        }
    }
}

/// Position of the connection in its high-level lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DriverState {
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
enum ReadPhase {
    /// Not yet read the 9 bytes of the next frame header.
    NeedHeader,
    /// Header read and validated; still collecting payload bytes. `total` is the full target
    /// fill (`FRAME_HEADER_LEN + payload_len`). The decoded header itself is cheap enough to
    /// re-parse from the buffer when we dispatch, so we don't stash it here.
    NeedPayload { total: usize },
}

/// Why the driver is closing — shaped around what the terminal `drive` result should be.
#[derive(Debug)]
enum CloseOutcome {
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
/// to know). Grows as later phases add state machine and flow-control bookkeeping.
#[derive(Debug)]
struct StreamEntry {
    /// Shared state (recv buffer, send slot, handler wakers). Owned by `Arc` so the
    /// handler task can outlive or operate concurrently with the driver's view.
    shared: Arc<StreamState>,

    /// Driver-private send-side state for an in-progress response. `None` until the conn
    /// task submits a response via [`H2Connection::submit_send`] and the driver picks it
    /// up on its next `service_handler_signals` tick.
    ///
    /// [`H2Connection::submit_send`]: super::H2Connection::submit_send
    send: Option<SendCursor>,

    /// Per-stream send flow-control window (RFC 9113 §6.9). Seeded from
    /// `peer_settings.effective_initial_window_size()` when the stream is opened;
    /// decremented as we emit DATA frames; incremented by peer
    /// `WINDOW_UPDATE(stream_id, inc)`; adjusted by `SETTINGS_INITIAL_WINDOW_SIZE` delta on
    /// mid-connection SETTINGS change (§6.9.2 — may drive negative). Overflow past
    /// [`MAX_FLOW_CONTROL_WINDOW`] is a stream-level `FLOW_CONTROL_ERROR` (→ `RST_STREAM`).
    send_window: i64,

    /// Per-stream recv flow-control window (RFC 9113 §6.9) — how many bytes we've told
    /// the peer it may still send on this stream. Starts at the server's advertised
    /// `SETTINGS_INITIAL_WINDOW_SIZE` (currently 0 — lazy-WU pattern); decremented as the
    /// peer's DATA frames arrive; incremented as we emit stream-level `WINDOW_UPDATE`
    /// (both the initial raise on the handler's `is_reading` signal and every subsequent
    /// refill crediting bytes the handler has consumed). A negative value means the peer
    /// overran the window — connection-level `FLOW_CONTROL_ERROR`.
    peer_recv_window: i64,
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

/// Why a stream transitioned to the closed state — dictates the error category for any
/// subsequent frame the peer sends on that stream id (RFC 9113 §5.1):
/// - `Reset`: closed via `RST_STREAM` (either direction). Subsequent frames → stream-level
///   `STREAM_CLOSED`.
/// - `EndStream`: closed via `END_STREAM` on both sides. Subsequent frames (other than
///   `WINDOW_UPDATE` / `PRIORITY` / `RST_STREAM`) → connection-level `STREAM_CLOSED`.
///
/// Streams that were never opened and are merely implicitly closed by a higher-id
/// HEADERS (§5.1.1) don't appear in the ledger; the fall-through case there is
/// connection-level `PROTOCOL_ERROR` per §5.1.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ClosedReason {
    Reset,
    EndStream,
}

/// Bounded FIFO of recently-closed streams and how they closed. Consulted when a peer
/// frame arrives on a stream id that's no longer in the active map to pick the right error
/// category per RFC 9113 §5.1.
///
/// Fixed cap — this is a correctness mechanism, not a concurrency-scaled structure. A
/// well-behaved peer never sends frames on a stream it knows is closed; the ledger only
/// needs to span a handful of RTTs between our close and a misbehaving peer's stale
/// frame. Oldest entries evict on overflow; evicted lookups fall through to the §5.1.1
/// connection-level `PROTOCOL_ERROR` default.
#[derive(Debug, Default)]
struct ClosedStreams {
    map: HashMap<u32, ClosedReason>,
    fifo: VecDeque<u32>,
}

impl ClosedStreams {
    const CAP: usize = 128;

    /// Record (or update) the close reason for `stream_id`. Idempotent on repeated calls
    /// for the same id; the most recent reason wins (a stream can be recorded as
    /// `EndStream` by `complete_and_remove_stream(Ok)` after already being recorded as
    /// `Reset` by `queue_rst_stream`, which is benign — the Reset recording is authoritative
    /// in that path because it happens first and the Ok path doesn't fire when the error
    /// path did).
    fn record(&mut self, stream_id: u32, reason: ClosedReason) {
        if self.map.insert(stream_id, reason).is_none() {
            self.fifo.push_back(stream_id);
            while self.fifo.len() > Self::CAP {
                if let Some(old) = self.fifo.pop_front() {
                    self.map.remove(&old);
                }
            }
        }
    }

    fn reason(&self, stream_id: u32) -> Option<ClosedReason> {
        self.map.get(&stream_id).copied()
    }
}

/// Result of dispatching one decoded frame.
enum Action {
    /// Frame handled; continue the main loop.
    Continue,
    /// A stream just opened and the request validated — return the [`Conn`] to the caller;
    /// the runtime adapter spawns a handler task per emitted Conn. Boxed to keep the enum
    /// small — `Conn` is over 500 bytes and most dispatches return `Continue`.
    Emit(Box<Conn<H2Transport>>),
    /// Begin graceful or erroring close with this outcome.
    Close(CloseOutcome),
}

impl<T> H2Driver<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    pub(super) fn new(connection: Arc<H2Connection>, transport: T, role: Role) -> Self {
        let shutting_down = connection.swansong().shutting_down();
        let config = AcceptorConfig::from_http_config(connection.context().config());
        Self {
            connection,
            transport,
            role,
            state: DriverState::AwaitingPreface,
            shutting_down,
            read_buf: vec![0u8; FRAME_HEADER_LEN],
            read_filled: 0,
            read_phase: ReadPhase::NeedHeader,
            write_buf: Vec::new(),
            write_cursor: 0,
            write_flush_pending: false,
            hpack: HpackDecoder::new(HPACK_TABLE_SIZE),
            streams: HashMap::new(),
            last_peer_stream_id: 0,
            pending_headers: None,
            close_outcome: None,
            finished: false,
            body_scratch: vec![0u8; MAX_DATA_CHUNK_SIZE as usize],
            connection_send_window: INITIAL_CONNECTION_RECV_WINDOW,
            connection_recv_window: INITIAL_CONNECTION_RECV_WINDOW,
            closed_streams: ClosedStreams::default(),
            config,
        }
    }

    /// The shared [`H2Connection`] this acceptor was created from.
    pub fn connection(&self) -> &Arc<H2Connection> {
        &self.connection
    }

    /// Drive the connection until the next request stream opens, the connection ends, or a
    /// fatal protocol or I/O error occurs.
    ///
    /// Returns `Ok(Some(conn))` for each new request stream — the runtime adapter is
    /// expected to spawn a handler task that consumes the [`Conn`]. Malformed requests
    /// (RFC 9113 §8.1.2) are handled internally with a stream-level `RST_STREAM` and never
    /// surfaced. Returns `Ok(None)` when the connection has been shut down cleanly (peer
    /// GOAWAY, our own swansong shutdown, peer EOF at a frame boundary).
    ///
    /// # Errors
    ///
    /// The returned future resolves to an [`H2Error`] for any *connection-level* protocol
    /// violation detected while decoding peer frames or for an unrecoverable transport I/O
    /// error. A final GOAWAY is sent before a protocol error is returned (best-effort; I/O
    /// errors skip it).
    // Mirrors `StreamExt::next` (a `&mut self -> impl Future<Output = Option<T>>` adapter),
    // not `Iterator::next`. The driver is also `Stream`, so callers can use either.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Next<'_, T> {
        Next { driver: self }
    }

    /// Poll-based driver core. Shared by [`Next`]'s `Future` impl, the [`Stream`] impl on
    /// [`H2Driver`], and [`H2Initiator`][super::H2Initiator]'s client-side Future impl.
    ///
    /// [`Stream`]: futures_lite::stream::Stream
    pub(super) fn drive(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Conn<H2Transport>, H2Error>>> {
        if self.finished {
            return Poll::Ready(None);
        }

        loop {
            // 1. Conn-task signals. Picks up window-update intent (`is_reading`) and new
            //    `submit_send` submissions, moving them into driver-private state.
            self.service_handler_signals();

            // 2. Send pump. Turns picked-up SendCursors into HEADERS / DATA / trailing- HEADERS
            //    frame bytes in `write_buf`. Body reads that return Pending leave the cursor in
            //    place — the body's source will wake the driver task.
            self.advance_outbound_sends(cx);

            // 3. Flush any pending outbound — never re-poll reads when we still owe bytes to the
            //    peer, and never signal closure to the caller before the wire is clean.
            match self.poll_flush_outbound(cx) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(e)) => {
                    // Flush failure while closing: just take whatever outcome we had and
                    // shelve the fresh I/O error. While running, record and finish.
                    if self.close_outcome.is_none() {
                        self.close_outcome = Some(CloseOutcome::Io(e));
                    }
                    return Poll::Ready(self.finish_with_current_outcome());
                }
                Poll::Pending => return Poll::Pending,
            }

            // 4. If we were closing, outbound is now drained. For graceful (or protocol-error)
            //    shutdowns, transition to `Drained` and wait for the peer to close its write half —
            //    otherwise the peer sees our drop as a reset rather than a clean close. For
            //    I/O-error shutdowns the transport is already untrustworthy, so skip the drain.
            if self.state == DriverState::Closing {
                if matches!(self.close_outcome, Some(CloseOutcome::Io(_))) {
                    return Poll::Ready(self.finish_with_current_outcome());
                }
                self.state = DriverState::Drained;
            }

            // 5. Server-initiated shutdown check. Only relevant while we're running — once we're
            //    past the Closing/Drained transition we've already committed to a close and
            //    re-observing the swansong here would re-enter begin_close in a loop. Post-shutdown
            //    re-polls of `ShuttingDown` are harmless themselves (event_listener-backed, not
            //    single-shot) but the re-entry isn't.
            if self.state == DriverState::Running
                && Pin::new(&mut self.shutting_down).poll(cx).is_ready()
            {
                self.begin_close(CloseOutcome::Graceful);
                continue;
            }

            // 6. State-specific step.
            match self.state {
                DriverState::AwaitingPreface => {
                    // Role-asymmetric: server reads the 24-byte preface off the wire; client
                    // writes it to `write_buf` (the next drain tick flushes it, then our
                    // SETTINGS, then the peer's SETTINGS arrives as the first frame in Running).
                    let poll = match self.role {
                        Role::Server => self.poll_read_preface(cx),
                        Role::Client => {
                            self.queue_client_preface();
                            Poll::Ready(Ok(()))
                        }
                    };
                    match poll {
                        Poll::Ready(Ok(())) => self.state = DriverState::NeedsServerSettings,
                        Poll::Ready(Err(e)) => {
                            self.close_outcome = Some(e);
                            return Poll::Ready(self.finish_with_current_outcome());
                        }
                        Poll::Pending => {
                            if self.park(cx) {
                                return Poll::Pending;
                            }
                        }
                    }
                }

                DriverState::NeedsServerSettings => {
                    self.queue_settings();
                    // §6.9.2 forbids SETTINGS from altering the connection-level flow-control
                    // window — it stays at the 65535 RFC baseline unless we raise it via
                    // `WINDOW_UPDATE(0)`. Do that immediately after SETTINGS so peer bulk
                    // uploads aren't capped at ~5 Mbit/s × RTT.
                    let raise = i64::from(self.config.initial_connection_window_size)
                        - INITIAL_CONNECTION_RECV_WINDOW;
                    if raise > 0 {
                        let raise = u32::try_from(raise).unwrap_or(u32::MAX);
                        self.queue_window_update(0, raise);
                        self.connection_recv_window += i64::from(raise);
                    }
                    self.state = DriverState::Running;
                }

                DriverState::Running => match self.poll_advance_read(cx) {
                    Poll::Ready(Ok(Action::Continue)) => {}
                    Poll::Ready(Ok(Action::Emit(conn))) => {
                        return Poll::Ready(Some(Ok(*conn)));
                    }
                    Poll::Ready(Ok(Action::Close(outcome))) => {
                        self.begin_close(outcome);
                    }
                    // Protocol errors need a GOAWAY on the wire before we terminate;
                    // `begin_close` queues that and transitions us to Closing so the next
                    // outer-loop iteration drains the frame. Io errors short-circuit with
                    // no GOAWAY (`begin_close` already skips queuing for those).
                    Poll::Ready(Err(e)) => {
                        self.begin_close(e);
                    }
                    Poll::Pending => {
                        if self.park(cx) {
                            return Poll::Pending;
                        }
                    }
                },

                DriverState::Closing => unreachable!("handled above once write_buf is drained"),

                DriverState::Drained => match self.poll_drain_peer(cx) {
                    Poll::Ready(()) => {
                        return Poll::Ready(self.finish_with_current_outcome());
                    }
                    Poll::Pending => return Poll::Pending,
                },
            }
        }
    }

    /// Register the driver's waker with the shared `outbound_waker` (so handler tasks can
    /// wake the driver) and tell the caller whether it's safe to park. Returns `true` if
    /// the driver should return `Poll::Pending`, or `false` if a handler produced work
    /// between our last check and the registration — in which case the caller should loop
    /// around to pick it up.
    fn park(&mut self, cx: &mut Context<'_>) -> bool {
        self.connection.outbound_waker().register(cx.waker());
        !self.has_pending_handler_signals()
    }

    /// Scan streams for conn-task-side signals that the driver should turn into driver-
    /// internal state. Three signals:
    /// - `recv.is_reading` (lazy `WINDOW_UPDATE`): conn task declared intent to read the request
    ///   body; emit a `WINDOW_UPDATE` topping the per-stream recv window up.
    /// - `send.submission` (response handoff): conn task called `submit_send`; move the submission
    ///   into the driver's private `SendCursor` so the next `advance_outbound_sends` tick can start
    ///   framing.
    /// - `pending_reset` (stream-error request): conn-task side (e.g. `ReceivedBody`'s
    ///   content-length guard) called
    ///   [`H2Connection::stream_error`][super::H2Connection::stream_error]; emit `RST_STREAM` and
    ///   clean the stream up via `complete_and_remove_stream`.
    ///
    /// Each stream's `StreamEntry` caches whether the corresponding driver-side action has
    /// already happened so we don't re-emit on every scan.
    fn service_handler_signals(&mut self) {
        // Pick up recv-side consumption + is-reading signals and turn them into
        // `WINDOW_UPDATE` frames. Two cases per stream:
        // - Handler has declared intent (is_reading) and the peer's recv window is still at the
        //   advertised SETTINGS baseline (≤ 0 by default, since we advertise 0): raise the stream
        //   window to [`MAX_STREAM_RECV_WINDOW`].
        // - Handler has drained `bytes_consumed` from its recv ring: emit a matching stream-level
        //   credit so the peer can keep sending, and aggregate across streams for a single
        //   connection-level `WINDOW_UPDATE(0)`.
        //
        // Collect stream_ids first to avoid holding &mut self.streams across `queue_*`
        // calls (which take &mut self). Short-lived Vec; bounded by MAX_CONCURRENT_STREAMS.
        let mut stream_updates: Vec<(u32, u32)> = Vec::new();
        let mut connection_credit: u64 = 0;
        let max_stream_recv_window = self.config.max_stream_recv_window_size;
        for (&id, entry) in &mut self.streams {
            // Initial lazy raise: peer hasn't been credited any recv window yet, handler
            // signaled intent, emit a one-time top-up to the stream target.
            if entry.peer_recv_window <= 0 && entry.shared.recv.is_reading.load(Ordering::Acquire) {
                stream_updates.push((id, max_stream_recv_window));
                entry.peer_recv_window += i64::from(max_stream_recv_window);
            }
            // Refill for bytes the handler has consumed since our last tick. Bounded by
            // MAX_FLOW_CONTROL_WINDOW (2^31-1) which comfortably fits u32; a handler that
            // somehow consumed more than u32::MAX bytes in one tick gets the credit
            // emitted in multiple frames on subsequent ticks.
            let consumed = entry.shared.recv.bytes_consumed.swap(0, Ordering::AcqRel);
            if consumed > 0 {
                let credit = u32::try_from(consumed).unwrap_or(u32::MAX);
                stream_updates.push((id, credit));
                entry.peer_recv_window += i64::from(credit);
                connection_credit = connection_credit.saturating_add(u64::from(credit));
            }
        }
        for (stream_id, increment) in stream_updates {
            self.queue_window_update(stream_id, increment);
        }
        if connection_credit > 0 {
            let credit = u32::try_from(connection_credit).unwrap_or(u32::MAX);
            self.queue_window_update(0, credit);
            self.connection_recv_window += i64::from(credit);
        }

        // Pick up new submissions. Iterate in place — `entry.send` is driver-private, no
        // borrow conflict with `self.write_buf`.
        for (&stream_id, entry) in &mut self.streams {
            if entry.send.is_some() {
                continue;
            }
            let submission = entry
                .shared
                .send
                .submission
                .lock()
                .expect("send submission mutex poisoned")
                .take();
            if let Some(submission) = submission {
                log::trace!("h2 stream {stream_id}: driver picked up submission");
                entry.send = Some(SendCursor::new(submission));
            }
        }

        // Pick up stream-error requests. Collect first, act second — same reason as the
        // window-advertise pass above.
        let resets: Vec<(u32, H2ErrorCode)> = self
            .streams
            .iter()
            .filter_map(|(&id, entry)| {
                entry
                    .shared
                    .pending_reset
                    .lock()
                    .expect("pending_reset mutex poisoned")
                    .take()
                    .map(|code| (id, code))
            })
            .collect();
        for (stream_id, code) in resets {
            log::debug!("h2 stream {stream_id}: conn-task-requested RST_STREAM({code:?})");
            self.queue_rst_stream(stream_id, code);
            self.complete_and_remove_stream(
                stream_id,
                Err(io::Error::other(format!(
                    "stream reset requested by conn task: {code:?}"
                ))),
            );
        }
    }

    /// True if any stream has a conn-task signal pending that we haven't yet serviced. Used
    /// by `park` to decide whether returning `Pending` is safe or whether we need to loop
    /// around.
    fn has_pending_handler_signals(&self) -> bool {
        self.streams.values().any(|e| {
            (e.peer_recv_window <= 0 && e.shared.recv.is_reading.load(Ordering::Acquire))
                || e.shared.recv.bytes_consumed.load(Ordering::Acquire) > 0
                || e.shared
                    .send
                    .submission
                    .lock()
                    .expect("send submission mutex poisoned")
                    .is_some()
                || e.shared
                    .pending_reset
                    .lock()
                    .expect("pending_reset mutex poisoned")
                    .is_some()
        })
    }

    /// Convert the current `close_outcome` into the terminal return of [`Self::drive`]. Must
    /// only be called after outbound bytes have been flushed. Graceful closes return `None`;
    /// errors surface as a final `Some(Err(...))` before subsequent polls return `None`.
    fn finish_with_current_outcome(&mut self) -> Option<Result<Conn<H2Transport>, H2Error>> {
        self.finished = true;
        match self.close_outcome.take() {
            None | Some(CloseOutcome::Graceful) => None,
            Some(CloseOutcome::Protocol(code)) => Some(Err(H2Error::Protocol(code))),
            Some(CloseOutcome::Io(e)) => Some(Err(H2Error::Io(e))),
        }
    }

    /// Enter the closing state: record the outcome and queue a GOAWAY (only for outcomes
    /// that warrant one). The main loop will drain `write_buf` and then finish.
    fn begin_close(&mut self, outcome: CloseOutcome) {
        log::trace!("h2 driver: begin_close({outcome:?})");
        // Don't overwrite a prior outcome (e.g. if an error fires in the middle of a
        // graceful shutdown, keep the error).
        let code = match &outcome {
            CloseOutcome::Graceful => Some(H2ErrorCode::NoError),
            CloseOutcome::Protocol(code) => Some(*code),
            CloseOutcome::Io(_) => None,
        };
        if self.close_outcome.is_none() {
            self.close_outcome = Some(outcome);
        }
        if let Some(code) = code {
            self.queue_goaway(self.last_peer_stream_id, code);
        }
        self.state = DriverState::Closing;
    }

    /// Read bytes from the transport into `read_buf[read_filled..target]` until
    /// `read_filled >= target`. Cancel-safe: if the caller drops the Future, any bytes
    /// already placed are preserved in the buffer.
    ///
    /// A 0-byte read is surfaced as `UnexpectedEof`. The caller maps this to a terminal
    /// I/O error; we don't emit a GOAWAY on peer-initiated close (consistent with the pre-
    /// poll driver).
    fn poll_fill_to(&mut self, target: usize, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.read_buf.len() < target {
            self.read_buf.resize(target, 0);
        }
        while self.read_filled < target {
            let n = ready!(
                Pin::new(&mut self.transport)
                    .poll_read(cx, &mut self.read_buf[self.read_filled..target])
            )?;
            if n == 0 {
                return Poll::Ready(Err(io::Error::from(io::ErrorKind::UnexpectedEof)));
            }
            self.read_filled += n;
        }
        Poll::Ready(Ok(()))
    }

    /// Post-GOAWAY, drain whatever inbound bytes are *immediately* available from the
    /// peer so our Drop sends a clean FIN (no unread data → no TCP RST) while the peer
    /// sees the GOAWAY we just emitted. Read loops internally: consume each Ready chunk,
    /// discard it, ask for more. Exits as soon as the transport returns `Pending` (no
    /// bytes available right now) OR `Ready(0)` (peer FIN already arrived) OR any error.
    ///
    /// Does **not** register the waker on `Pending` — we're actively closing, not
    /// observing the peer. A peer that happens to send more bytes after our exit will
    /// have those bytes dropped when the transport is closed; that's a race the peer
    /// chose to lose by sending after receiving our GOAWAY.
    ///
    /// Returning `Ready(())` unconditionally (no `Pending` case) lets the caller finalize
    /// immediately. The `Poll` wrapper is kept for symmetry with the rest of the driver's
    /// poll-style methods.
    fn poll_drain_peer(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        // A peer flooding us with bytes could keep this loop going a long time. Cap it
        // so a pathological client can't pin our close-out forever.
        const MAX_DISCARD_ITERATIONS: usize = 256;
        // Lightweight scratch — we're throwing it away. 512 balances "drain in few
        // iterations" against "don't hold a large buffer for a rare path."
        let mut scratch = [0u8; 512];
        for _ in 0..MAX_DISCARD_ITERATIONS {
            // We pass `cx` through for the benefit of the transport's `poll_read` contract,
            // but we *interpret* `Pending` as "done draining" rather than parking on it —
            // we're actively closing, not observing. A peer that sends more bytes after
            // our exit loses the race.
            match Pin::new(&mut self.transport).poll_read(cx, &mut scratch) {
                Poll::Ready(Ok(0) | Err(_)) | Poll::Pending => {
                    return Poll::Ready(());
                }
                Poll::Ready(Ok(_)) => {}
            }
        }
        Poll::Ready(())
    }

    /// Drain `write_buf[write_cursor..]` to the transport, then flush if bytes were
    /// written. Returns `Ready(Ok(()))` when both the buffer is empty AND any pending
    /// flush has completed.
    fn poll_flush_outbound(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.write_cursor < self.write_buf.len() {
            let n = ready!(
                Pin::new(&mut self.transport).poll_write(cx, &self.write_buf[self.write_cursor..])
            )?;
            if n == 0 {
                return Poll::Ready(Err(io::Error::from(io::ErrorKind::WriteZero)));
            }
            self.write_cursor += n;
        }
        // Fully drained — reset the buffer so future writes start at offset 0.
        self.write_buf.clear();
        self.write_cursor = 0;
        if self.write_flush_pending {
            ready!(Pin::new(&mut self.transport).poll_flush(cx))?;
            self.write_flush_pending = false;
        }
        Poll::Ready(Ok(()))
    }

    // --- outbound frame queuing helpers --------------------------------------------------
    //
    // All `queue_*` helpers append encoded bytes to `write_buf` via [`Self::queue_frame`]
    // and set `write_flush_pending`. The driver's main loop drains `write_buf` before
    // observing progress elsewhere.

    /// Append one frame to `write_buf`. `max_len` must be an upper bound on the encoded
    /// size; `encode` writes into the provided slice and returns the actual length (panics
    /// via `expect` if the caller under-sized `max_len`).
    fn queue_frame(&mut self, max_len: usize, encode: impl FnOnce(&mut [u8]) -> Option<usize>) {
        let start = self.write_buf.len();
        self.write_buf.resize(start + max_len, 0);
        let n = encode(&mut self.write_buf[start..]).expect("buffer sized from max_len");
        self.write_buf.truncate(start + n);
        self.write_flush_pending = true;
    }

    fn queue_settings(&mut self) {
        let settings = H2Settings::from_config(self.connection.context().config());
        self.queue_frame(frame::settings::encoded_len(&settings), |buf| {
            frame::settings::encode(&settings, buf)
        });
    }

    /// Append the 24-byte RFC 9113 §3.4 client connection preface to `write_buf`. The
    /// next outbound drain flushes it, and the `NeedsServerSettings` state follows up
    /// with our initial SETTINGS frame. Client role only.
    fn queue_client_preface(&mut self) {
        self.write_buf.extend_from_slice(recv::CLIENT_PREFACE);
        self.write_flush_pending = true;
    }

    fn queue_settings_ack(&mut self) {
        self.queue_frame(
            frame::settings::ACK_ENCODED_LEN,
            frame::settings::encode_ack,
        );
    }

    fn queue_ping_ack(&mut self, opaque_data: [u8; 8]) {
        self.queue_frame(frame::ping::ENCODED_LEN, |buf| {
            frame::ping::encode(opaque_data, true, buf)
        });
    }

    fn queue_window_update(&mut self, stream_id: u32, increment: u32) {
        self.queue_frame(frame::window_update::ENCODED_LEN, |buf| {
            frame::window_update::encode(stream_id, increment, buf)
        });
    }

    fn queue_goaway(&mut self, last_stream_id: u32, code: H2ErrorCode) {
        self.queue_frame(frame::goaway::encoded_len(0), |buf| {
            frame::goaway::encode(last_stream_id, code, &[], buf)
        });
    }

    fn queue_rst_stream(&mut self, stream_id: u32, code: H2ErrorCode) {
        self.queue_frame(frame::rst_stream::ENCODED_LEN, |buf| {
            frame::rst_stream::encode(stream_id, code, buf)
        });
        // Record in the ledger so subsequent frames the peer sends on this stream get
        // stream-level `STREAM_CLOSED` rather than connection-level `PROTOCOL_ERROR`
        // (§5.1 closed-state rule for RST_STREAM-closed streams).
        self.closed_streams.record(stream_id, ClosedReason::Reset);
    }

    /// Look up why a stream is closed. `None` means either never-opened or evicted from the
    /// bounded ledger — both fall through to the connection-level §5.1.1 default.
    pub(super) fn closed_reason(&self, stream_id: u32) -> Option<ClosedReason> {
        self.closed_streams.reason(stream_id)
    }
}

/// Future returned by [`H2Driver::next`]. Resolves to `None` on graceful close, `Some(Ok)`
/// when a new request stream opens, or `Some(Err)` on a fatal protocol or I/O error.
#[must_use = "futures do nothing unless awaited"]
#[derive(Debug)]
pub struct Next<'a, T> {
    driver: &'a mut H2Driver<T>,
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

impl<T> futures_lite::stream::Stream for H2Driver<T>
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
fn frame_slice(buf: &[u8], start: usize, length: u32, total: usize) -> Result<&[u8], CloseOutcome> {
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
