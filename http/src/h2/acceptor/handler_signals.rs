//! Conn-task → driver work-pickup boundary.
//!
//! Handler tasks raise the per-stream `needs_servicing` mailbox flag whenever they produce
//! work the driver should service: a new submission, a reset request, a release request, a
//! `bytes_consumed` increment, or a first `is_reading` transition. The driver's
//! [`service_handler_signals`][H2Driver::service_handler_signals] tick walks every stream,
//! consults the mailbox via `swap(false)`, and only pays for the per-field pickup when the
//! flag was set — idle streams cost a single atomic RMW per tick.
//!
//! [`pick_up_new_client_streams`][H2Driver::pick_up_new_client_streams] is the client-role
//! companion: streams the conn task has published via
//! [`H2Connection::open_stream`][crate::h2::H2Connection::open_stream] are present in the
//! shared map but absent from the driver's private `streams` map until this pass promotes
//! them.

use super::{H2Driver, Role, StreamEntry, send::SendCursor};
use crate::h2::{H2ErrorCode, transport::StreamState};
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    io,
    sync::{Arc, atomic::Ordering},
};

impl<T> H2Driver<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    /// Scan streams for conn-task-side signals that the driver should turn into driver-
    /// internal state. Five per-stream signals, all gated by the
    /// [`StreamState::needs_servicing`][StreamState] mailbox flag:
    /// - `recv.is_reading` (lazy `WINDOW_UPDATE`): conn task declared intent to read the request
    ///   body; emit a `WINDOW_UPDATE` topping the per-stream recv window up.
    /// - `recv.bytes_consumed`: handler drained N bytes from the recv ring; emit `WINDOW_UPDATE`
    ///   credit at both stream and connection levels.
    /// - `send.submission` (response handoff): conn task called `submit_send`; move the submission
    ///   into the driver's private `SendCursor` so the next `advance_outbound_sends` tick can start
    ///   framing.
    /// - `pending_reset` (stream-error request): conn-task side (e.g. `ReceivedBody`'s
    ///   content-length guard) called
    ///   [`H2Connection::stream_error`][crate::h2::H2Connection::stream_error]; emit `RST_STREAM`
    ///   and clean the stream up via `complete_and_remove_stream`.
    /// - `pending_release`: client-role `H2Transport::Drop` on a wire-closed-but-held stream;
    ///   remove from both stream maps without emitting `RST_STREAM`.
    ///
    /// Idle streams (mailbox flag `false`) are skipped after a single atomic RMW —
    /// avoids 4+ mutex acquires per stream per tick.
    pub(super) fn service_handler_signals(&mut self) {
        // Client role: pick up streams the conn task has opened via `H2Connection::open_stream`
        // — they're in the shared map but not yet in our private `self.streams`. After this
        // pass the consolidated per-stream walk below will promote each new stream's staged
        // submission into a `SendCursor` on the same tick.
        self.pick_up_new_client_streams();

        // Pick up any opaque payloads queued by `H2Connection::send_ping` and emit them as
        // outbound `PING { ack: false }` frames. The corresponding ACKs (handled in recv)
        // complete the awaiting futures and record their RTTs.
        for opaque in self.connection.drain_pending_ping_outbound() {
            self.queue_active_ping(opaque);
        }

        // Single per-stream walk gated by the `needs_servicing` mailbox flag. Conn-task code
        // raises the flag whenever it produces work; we clear via `swap(false)` and run the
        // per-field pickup only when set. Idle streams cost one atomic RMW per tick instead of
        // four mutex acquires.
        //
        // Collect work into short-lived Vecs (bounded by MAX_CONCURRENT_STREAMS) so we can act
        // on it with `&mut self` after releasing the borrow on `self.streams`.
        let mut stream_updates: Vec<(u32, u32)> = Vec::new();
        let mut connection_credit: u64 = 0;
        let mut resets: Vec<(u32, H2ErrorCode)> = Vec::new();
        let mut releases: Vec<u32> = Vec::new();
        let max_stream_recv_window = self.config.max_stream_recv_window_size();

        for (&id, entry) in &mut self.streams {
            // Mailbox-flag fast-skip. Idle streams stop here.
            if !entry.shared.needs_servicing.swap(false, Ordering::AcqRel) {
                continue;
            }

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

            // New submission pickup.
            if entry.send.is_none() {
                let submission = entry
                    .shared
                    .send
                    .submission
                    .lock()
                    .expect("send submission mutex poisoned")
                    .take();
                if let Some(submission) = submission {
                    log::trace!("h2 stream {id}: driver picked up submission");
                    entry.send = Some(SendCursor::new(submission));
                }
            }

            // Conn-task-requested RST_STREAM.
            if let Some(code) = entry
                .shared
                .pending_reset
                .lock()
                .expect("pending_reset mutex poisoned")
                .take()
            {
                resets.push((id, code));
            }

            // Application-side release for client-role wire-closed-but-held streams. The
            // `H2Transport::Drop` for a cleanly-completed stream sets `pending_release`;
            // we remove the entry from both stream maps below. No `RST_STREAM` — the wire
            // is already closed.
            if entry.shared.pending_release.swap(false, Ordering::AcqRel) {
                releases.push(id);
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
        for stream_id in releases {
            log::trace!("h2 stream {stream_id}: application released held stream — removing");
            self.remove_from_stream_maps(stream_id);
        }
    }

    /// True if any stream has a conn-task signal pending that we haven't yet serviced. Used
    /// by `park` to decide whether returning `Pending` is safe or whether we need to loop
    /// around.
    pub(super) fn has_pending_handler_signals(&self) -> bool {
        // Client role: a stream the conn task has opened (added to the shared map) but the
        // driver hasn't yet picked up isn't represented in `self.streams` and thus would be
        // invisible to the per-stream checks below. Without this guard, an `open_stream`
        // call landing between `service_handler_signals` and `park`'s waker registration
        // could deadlock — the waker.wake fires before any task has registered, and the
        // pickup work isn't on `self.streams` yet for the registered waker to detect.
        if self.role == Role::Client {
            let shared = self.connection.streams_lock();
            if shared.keys().any(|id| !self.streams.contains_key(id)) {
                return true;
            }
        }
        // Mailbox-flag check — conn-task code raises `needs_servicing` whenever it produces
        // work the driver should service. One atomic load per stream replaces the previous
        // multi-atomic + multi-mutex peek.
        self.streams
            .values()
            .any(|e| e.shared.needs_servicing.load(Ordering::Acquire))
    }

    /// Client role: scan [`H2Connection::streams`][crate::h2::H2Connection] for ids the conn
    /// task has published via [`H2Connection::open_stream`][crate::h2::H2Connection::open_stream]
    /// that we don't yet have a [`StreamEntry`] for, and create one per id seeded with the
    /// peer-advertised initial send window and our own advertised initial recv window.
    /// No-op for server role (server streams are created by inbound HEADERS in
    /// [`finalize_new_request_stream`][super::recv]).
    fn pick_up_new_client_streams(&mut self) {
        if self.role != Role::Client {
            return;
        }
        // Collect first so we don't hold the shared streams lock across `streams.insert`
        // (no actual deadlock risk, but keeps the lock as short as possible).
        let new_streams: Vec<(u32, Arc<StreamState>)> = {
            let shared = self.connection.streams_lock();
            shared
                .iter()
                .filter(|(id, _)| !self.streams.contains_key(id))
                .map(|(&id, s)| (id, Arc::clone(s)))
                .collect()
        };
        if new_streams.is_empty() {
            return;
        }
        let send_window = i64::from(
            self.connection
                .current_peer_settings()
                .effective_initial_window_size(),
        );
        let peer_recv_window = i64::from(self.config.initial_stream_window_size());
        for (id, shared) in new_streams {
            log::trace!("h2 client: driver picked up new client-opened stream {id}");
            self.streams
                .insert(id, StreamEntry::new(shared, send_window, peer_recv_window));
        }
    }
}
