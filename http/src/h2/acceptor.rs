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
//! Driver impl is split across this file and child modules to keep each focused:
//!
//! - **`acceptor.rs`** (this file): struct definition, the [`Self::drive`] orchestration loop, I/O
//!   read primitives (`poll_fill_to`, `poll_drain_peer`), and the supporting enums
//!   ([`DriverState`], [`ReadPhase`], [`CloseOutcome`], [`Action`], [`StreamEntry`]).
//! - **`acceptor::closed_streams`**: bounded ledger of recently-closed streams + reasons, consulted
//!   to pick the right §5.1 error category for stale peer frames.
//! - **`acceptor::handler_signals`**: conn-task → driver work-pickup boundary. Owns the
//!   `needs_servicing` mailbox protocol — `service_handler_signals`, `pick_up_new_client_streams`,
//!   `has_pending_handler_signals`.
//! - **`acceptor::outbound`**: outbound write/flush plumbing and `queue_*` frame helpers.
//! - **`acceptor::recv`**: receive side — frame reader, dispatch, HEADERS+CONTINUATION
//!   accumulation, malformed-request `RST_STREAM`, DATA routing into per-stream recv rings.
//! - **`acceptor::send`**: send pump — picks up [`SendCursor`][send::SendCursor]s from the
//!   conn-task signal pickup, frames HEADERS / DATA / trailing-HEADERS, signals completion.
//!
//! [`H2Connection::run`]: super::H2Connection::run
//! [`Stream`]: futures_lite::stream::Stream

mod closed_streams;
mod constants;
mod handler_signals;
mod outbound;
mod recv;
mod send;
#[cfg(test)]
mod tests;
mod types;

use super::{
    H2Error, H2ErrorCode, connection::H2Connection, frame::FRAME_HEADER_LEN, role::Role,
    transport::H2Transport,
};
use crate::{
    Conn,
    headers::hpack::{HpackDecoder, HpackEncoder},
};
use closed_streams::{ClosedReason, ClosedStreams};
use constants::{
    INITIAL_CONNECTION_RECV_WINDOW, MAX_BUFFER_SIZE, MAX_DATA_CHUNK_SIZE, MAX_FLOW_CONTROL_WINDOW,
};
use futures_lite::io::{AsyncRead, AsyncWrite};
use hashbrown::HashMap;
use recv::PendingHeaders;
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, ready},
};
use swansong::ShuttingDown;
use types::{
    AcceptorConfig, Action, CloseOutcome, DriverState, Next, ReadPhase, StreamEntry, frame_slice,
};

/// Owns the per-connection TCP transport and drives the HTTP/2 demux loop.
///
/// See the module docs for the high-level driver shape and how its impl is split across the
/// `recv` and `send` child modules.
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

    /// HPACK encoder state. The driver is the sole owner — handlers / conn tasks
    /// no longer touch it, so this is a plain field with no synchronization.
    hpack_encoder: HpackEncoder,

    /// Per-stream state, keyed by stream id. Driver-only — handler tasks hold their own
    /// `Arc<StreamState>` via [`H2Transport`] and don't consult this table. The entry
    /// bundles the shared state with driver-private bookkeeping (e.g. "have we already
    /// advertised the recv window after seeing `is_reading`?").
    streams: HashMap<u32, StreamEntry>,

    /// Highest peer-initiated stream id seen so far. Peer-initiated (client) stream ids
    /// must be odd and strictly increasing.
    last_peer_stream_id: u32,

    /// Accumulator for an in-progress HEADERS block that is waiting on further CONTINUATION
    /// frames. `None` outside a HEADERS block. The spec forbids any frame on any stream
    /// from interleaving while this is `Some`.
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

    /// Connection-level send flow-control window. Tracked as [`i64`] so mid-connection
    /// `INITIAL_WINDOW_SIZE` reductions can drive per-stream windows temporarily negative
    /// — kept here to the connection window for symmetry though the connection window
    /// itself is *not* affected by `SETTINGS_INITIAL_WINDOW_SIZE`. Decremented as we emit
    /// DATA; incremented by peer `WINDOW_UPDATE(stream_id=0, inc)`. Overflow past
    /// [`MAX_FLOW_CONTROL_WINDOW`] is a connection-level `FLOW_CONTROL_ERROR`.
    connection_send_window: i64,

    /// Connection-level recv flow-control window. Starts at the spec's baseline of 65535
    /// octets and is raised to [`MAX_CONNECTION_RECV_WINDOW`] via an initial
    /// `WINDOW_UPDATE(0)` right after SETTINGS — the spec forbids SETTINGS from altering
    /// it, so WU is the only path. Decremented as peer DATA frames arrive (across all
    /// streams); incremented as the handler-task-side consumption signal is picked up and
    /// we emit `WINDOW_UPDATE(0, consumed)`. A negative value means the peer overran the
    /// window — connection-level `FLOW_CONTROL_ERROR`.
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

impl<T> H2Driver<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    pub(super) fn new(connection: Arc<H2Connection>, transport: T, role: Role) -> Self {
        let shutting_down = connection.swansong().shutting_down();
        let context = connection.context();
        let config = AcceptorConfig::from_http_config(context.config());
        let hpack_encoder = HpackEncoder::new(
            context.observer.clone(),
            context.config.dynamic_table_capacity(),
            context.config.recent_pairs_size(),
        );
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
            hpack: HpackDecoder::new(config.hpack_table_capacity()),
            hpack_encoder,
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
    /// expected to spawn a handler task that consumes the [`Conn`]. Malformed requests are
    /// handled internally with a stream-level `RST_STREAM` and never surfaced. Returns
    /// `Ok(None)` when the connection has been shut down cleanly (peer GOAWAY, our own
    /// swansong shutdown, peer EOF at a frame boundary).
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
    #[allow(
        clippy::too_many_lines,
        reason = "state-machine orchestration; splitting muddies the read-as-a-recipe shape"
    )]
    pub(super) fn drive(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Conn<H2Transport>, H2Error>>> {
        if self.finished {
            return Poll::Ready(None);
        }

        for loop_number in 0..self.config.copy_loops_per_yield() {
            log::trace!("h2 drive loop number: {loop_number}");
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
            //
            //    Defer the transition while in-flight streams still have outbound (SendCursor
            //    not yet `Complete`) OR inbound (`recv.eof` not yet set) work. Without this, a
            //    handler that submits trailers *after* the cancellation race resolves (gRPC
            //    `Cancellation::race`) gets stranded with bytes parked in mailboxes, and a
            //    client receiving GOAWAY mid-stream stops decoding incoming frames before the
            //    server's trailing HEADERS arrive. Falls through to step 6 so the recv pump
            //    (also gated on Running|Closing now) keeps running and parks on the transport
            //    read waker rather than the outbound-only `park` here.
            if self.state == DriverState::Closing {
                if matches!(self.close_outcome, Some(CloseOutcome::Io(_))) {
                    return Poll::Ready(self.finish_with_current_outcome());
                }
                if self.has_active_send_cursors() || self.has_pending_recv() {
                    self.log_closing_blockers();
                } else {
                    self.set_state(
                        DriverState::Drained,
                        "outbound drained, no in-flight streams",
                    );
                }
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
                        Poll::Ready(Ok(())) => {
                            self.set_state(DriverState::NeedsServerSettings, "preface complete");
                        }
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
                    // The spec forbids SETTINGS from altering the connection-level
                    // flow-control window — it stays at the 65535 baseline unless we raise
                    // it via `WINDOW_UPDATE(0)`. Do that immediately after SETTINGS so peer
                    // bulk uploads aren't capped at ~5 Mbit/s × RTT.
                    let raise = i64::from(self.config.initial_connection_window_size())
                        - INITIAL_CONNECTION_RECV_WINDOW;
                    if raise > 0 {
                        let raise = u32::try_from(raise).unwrap_or(u32::MAX);
                        self.queue_window_update(0, raise);
                        self.connection_recv_window += i64::from(raise);
                    }
                    self.set_state(DriverState::Running, "initial SETTINGS queued");
                }

                // Read pump runs in both Running and Closing so a Closing-side driver
                // (we sent or received GOAWAY) keeps decoding inbound frames for streams
                // that haven't reached `recv.eof` yet — e.g. trailing HEADERS for an
                // in-flight server-stream the peer is about to send. New `Action::Emit`
                // streams are ignored in Closing: post-GOAWAY the peer shouldn't be
                // opening new ones (and we wouldn't want to dispatch handlers for them
                // even if it did).
                DriverState::Running | DriverState::Closing => match self.poll_advance_read(cx) {
                    Poll::Ready(Ok(Action::Continue)) => {}
                    Poll::Ready(Ok(Action::Emit(conn))) => {
                        if self.state == DriverState::Running {
                            return Poll::Ready(Some(Ok(*conn)));
                        }
                        // Closing — drop the conn; outer loop continues processing
                        // remaining in-flight streams until drained.
                    }
                    Poll::Ready(Ok(Action::Close(outcome))) => {
                        self.begin_close(outcome);
                    }
                    // Protocol errors need a GOAWAY on the wire before we terminate;
                    // `begin_close` queues that and transitions us to Closing so the next
                    // outer-loop iteration drains the frame. Io errors short-circuit:
                    // if we're already Closing, the transport is gone, so finish without
                    // looping forever waiting for in-flight streams (`has_pending_recv`
                    // can't decide on its own that the peer is never sending again).
                    Poll::Ready(Err(e)) => {
                        if self.state == DriverState::Closing {
                            self.close_outcome.get_or_insert(e);
                            return Poll::Ready(self.finish_with_current_outcome());
                        }
                        self.begin_close(e);
                    }
                    Poll::Pending => {
                        if self.park(cx) {
                            return Poll::Pending;
                        }
                    }
                },

                DriverState::Drained => match self.poll_drain_peer(cx) {
                    Poll::Ready(()) => {
                        return Poll::Ready(self.finish_with_current_outcome());
                    }
                    Poll::Pending => return Poll::Pending,
                },
            }
        }

        // Cooperative yield: we made `copy_loops_per_yield` rounds of progress without
        // hitting an internal Pending. Re-arm immediately and let the runtime pick up
        // anything else it has waiting before we resume.
        cx.waker().wake_by_ref();
        Poll::Pending
    }

    /// Register the driver's waker with the shared `outbound_waker` (so handler tasks can
    /// wake the driver) and tell the caller whether it's safe to park. Returns `true` if
    /// the driver should return `Poll::Pending`, or `false` if a handler produced work
    /// between our last check and the registration — in which case the caller should loop
    /// around to pick it up.
    fn park(&mut self, cx: &mut Context<'_>) -> bool {
        self.connection.outbound_waker().register(cx.waker());
        !self.has_pending_handler_signals() && !self.has_pending_outbound_progress()
    }

    /// Convert the current `close_outcome` into the terminal return of [`Self::drive`]. Must
    /// only be called after outbound bytes have been flushed. Graceful closes return `None`;
    /// errors surface as a final `Some(Err(...))` before subsequent polls return `None`.
    fn finish_with_current_outcome(&mut self) -> Option<Result<Conn<H2Transport>, H2Error>> {
        self.finished = true;
        // Complete every outstanding `H2Connection::send_ping` future with an error so
        // awaiting callers don't block forever. Safe to call regardless of outcome —
        // a no-op if no pings are in flight.
        self.connection.fail_pending_pings(
            io::ErrorKind::ConnectionAborted,
            "h2 connection closed before PING ACK",
        );
        // Wake any `PeerSettings` waiters so a peer that disconnects without ever sending
        // SETTINGS doesn't strand them. Their `poll` rechecks swansong state and returns
        // Ready; the caller's follow-up operation surfaces the connection-closed error.
        self.connection.wake_peer_settings_waiters();
        // Resolve every still-live stream's recv-side waiters. A connection that dies with
        // an in-flight stream (server GOAWAY + close, peer FIN, I/O error) leaves any task
        // parked on the response — `response_headers`, a body `poll_read`, an upgrade
        // `poll_write` — with no other wake source. Without this a client request hangs
        // forever on a graceful server shutdown. Mirror the per-stream RST teardown:
        // terminal `Reset` (recv reports eof → `ResponseHeaders` yields `ConnectionAborted`,
        // reads return EOF, writes `BrokenPipe`) + the same waker fan-out.
        let reset_code = match &self.close_outcome {
            Some(CloseOutcome::Protocol(code)) => *code,
            _ => H2ErrorCode::NoError,
        };
        for entry in self.streams.values() {
            {
                let mut lifecycle = entry.shared.lifecycle_lock();
                if !matches!(&*lifecycle, crate::h2::lifecycle::StreamLifecycle::Reset(_)) {
                    *lifecycle = crate::h2::lifecycle::StreamLifecycle::Reset(reset_code);
                }
            }
            entry.shared.recv.waker.wake();
            entry.shared.recv.response_headers_waker.wake();
            entry.shared.send.outbound_write_waker.wake();
        }
        match self.close_outcome.take() {
            None | Some(CloseOutcome::Graceful) => None,
            Some(CloseOutcome::Protocol(code)) => Some(Err(H2Error::Protocol(code))),
            Some(CloseOutcome::Io(e)) => Some(Err(H2Error::Io(e))),
        }
    }

    /// Enter the closing state: record the outcome and queue a GOAWAY (only for outcomes
    /// that warrant one). The main loop will drain `write_buf` and then finish.
    fn begin_close(&mut self, outcome: CloseOutcome) {
        // Idempotent: with the recv pump now running in Closing (so we keep
        // decoding inbound frames for in-flight streams across GOAWAY), a peer
        // GOAWAY arriving after we've already begun closing would otherwise
        // re-queue our own GOAWAY and re-enter Closing, ping-ponging forever
        // with a peer that mirrors the behavior.
        if self.state == DriverState::Closing || self.state == DriverState::Drained {
            log::trace!(
                "h2 driver: begin_close({outcome:?}) — already in {:?}, ignoring",
                self.state,
            );
            return;
        }
        // Don't overwrite a prior outcome (e.g. if an error fires in the middle of a
        // graceful shutdown, keep the error).
        let code = match &outcome {
            CloseOutcome::Graceful => Some(H2ErrorCode::NoError),
            CloseOutcome::Protocol(code) => Some(*code),
            CloseOutcome::Io(_) => None,
        };
        let reason = match &outcome {
            CloseOutcome::Graceful => "graceful close",
            CloseOutcome::Protocol(_) => "protocol error",
            CloseOutcome::Io(_) => "i/o error",
        };
        if self.close_outcome.is_none() {
            self.close_outcome = Some(outcome);
        }
        if let Some(code) = code {
            self.queue_goaway(self.last_peer_stream_id, code);
        }
        self.set_state(DriverState::Closing, reason);
    }

    /// The sole mutator of `self.state`. Logs every transition so a trace log reads as
    /// a sequence of named lifecycle events.
    fn set_state(&mut self, new: DriverState, reason: &'static str) {
        if self.state == new {
            return;
        }
        log::trace!(
            "h2 driver: state {old:?} → {new:?} ({reason})",
            old = self.state,
        );
        self.state = new;
    }

    /// Log which in-flight streams are blocking the `Closing → Drained` transition.
    /// Called from the closing-state check when at least one predicate (`has_active_send_cursors`
    /// or `has_pending_recv`) is still true, so a trace log shows exactly which streams the
    /// driver is waiting on.
    fn log_closing_blockers(&self) {
        if !log::log_enabled!(log::Level::Trace) {
            return;
        }
        for (id, entry) in &self.streams {
            let lifecycle = entry.shared.lifecycle_lock();
            if entry.send.is_some() || lifecycle.has_active_send() || lifecycle.has_pending_recv() {
                log::trace!(
                    "h2 driver: Closing — stream {id} blocking drain (lifecycle={lifecycle:?}, \
                     cursor_present={})",
                    entry.send.is_some(),
                );
            }
        }
    }

    /// Read bytes from the transport into `read_buf[read_filled..target]` until
    /// `read_filled >= target`. Cancel-safe: if the caller drops the Future, any bytes
    /// already placed are preserved in the buffer.
    ///
    /// A 0-byte read is surfaced as `UnexpectedEof`. The caller maps this to a terminal
    /// I/O error; we don't emit a GOAWAY on peer-initiated close.
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

    /// Look up why a stream is closed. `None` means either never-opened or evicted from the
    /// bounded ledger — both fall through to the connection-level default.
    pub(super) fn closed_reason(&self, stream_id: u32) -> Option<ClosedReason> {
        self.closed_streams.reason(stream_id)
    }
}
