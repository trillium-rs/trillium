//! Conn-task → driver work-pickup boundary.
//!
//! Handler tasks raise the per-stream `needs_servicing` mailbox flag as a side effect of normal
//! operation; the driver's [`service_handler_signals`][H2Driver::service_handler_signals] tick
//! consults the mailbox per stream, so idle streams cost a single atomic RMW per tick.

use super::{H2Driver, Role, StreamEntry, send::SendCursor};
use crate::h2::transport::StreamState;
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::sync::{Arc, atomic::Ordering};

impl<T> H2Driver<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    /// Walk every active stream, consult the per-stream mailbox, and move each signal into
    /// driver-internal state. Three classes of signal:
    ///
    /// - **Recv flow control** (`recv.is_reading`, `recv.bytes_consumed`): emits `WINDOW_UPDATE`
    ///   frames to top up the per-stream and connection-level recv windows.
    /// - **Outbound parts** (`send.queue`): drained into the stream's driver-private
    ///   [`SendCursor`]; the send pump frames them. (`Reset` rides the queue too — it's framed by
    ///   the pump, not picked up here.)
    /// - **Application release** (`send.transport_dropped`): the handler dropped a wire-closed
    ///   stream it was still holding for trailer access; remove it from both maps.
    /// - **PING outbound queue** (connection-level, not per-stream): drain and queue any pending
    ///   PING frames.
    pub(super) fn service_handler_signals(&mut self) {
        // Must run before the per-stream walk so new streams' parts are picked up on the same tick.
        self.pick_up_new_client_streams();

        for opaque in self.connection.drain_pending_ping_outbound() {
            self.queue_active_ping(opaque);
        }

        let mut stream_updates: Vec<(u32, u32)> = Vec::new();
        let mut connection_credit: u64 = 0;
        let mut releases: Vec<u32> = Vec::new();
        let max_stream_recv_window = self.config.max_stream_recv_window_size();

        for (&id, entry) in &mut self.streams {
            if !entry.shared.needs_servicing.swap(false, Ordering::AcqRel) {
                continue;
            }

            // Recv-side flow control. `is_reading` and `bytes_consumed` track the handler's read
            // cadence on a normal request body. First credit: peer hasn't been credited any recv
            // window yet and the handler signaled it's reading.
            if entry.peer_recv_window <= 0 && entry.shared.recv.is_reading.load(Ordering::Acquire) {
                stream_updates.push((id, max_stream_recv_window));
                entry.peer_recv_window += i64::from(max_stream_recv_window);
            }

            let consumed = entry.shared.recv.bytes_consumed.swap(0, Ordering::AcqRel);
            if consumed > 0 {
                let credit = u32::try_from(consumed).unwrap_or(u32::MAX);
                stream_updates.push((id, credit));
                entry.peer_recv_window += i64::from(credit);
                connection_credit = connection_credit.saturating_add(u64::from(credit));
            }

            // Drain staged outbound parts into the driver-private cursor.
            {
                let mut queue = entry
                    .shared
                    .send
                    .queue
                    .lock()
                    .expect("send queue mutex poisoned");
                if !queue.is_empty() {
                    let cursor = entry.send.get_or_insert_with(SendCursor::default);
                    for part in queue.drain(..) {
                        cursor.stage_part(part);
                    }
                }
            }

            // Application released a wire-closed-but-held stream.
            if entry.shared.send.transport_dropped.load(Ordering::Acquire) {
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
        for stream_id in releases {
            log::trace!("h2 stream {stream_id}: application released held stream — removing");
            self.remove_from_stream_maps(stream_id);
        }
    }

    /// True if any stream has a conn-task signal pending that we haven't yet serviced. Used by
    /// `park` to decide whether returning `Pending` is safe or whether we need to loop around.
    pub(super) fn has_pending_handler_signals(&self) -> bool {
        // Client-role guard: streams the conn task just opened are in the shared map but not yet in
        // `self.streams`. Without this, an `open_stream` landing between `service_handler_signals`
        // and `park`'s waker registration would deadlock — the wake fires before the waker is
        // registered.
        if self.role == Role::Client {
            let shared = self.connection.streams_lock();
            if shared.keys().any(|id| !self.streams.contains_key(id)) {
                return true;
            }
        }
        self.streams
            .values()
            .any(|e| e.shared.needs_servicing.load(Ordering::Acquire))
    }

    /// Client role: scan [`H2Connection::streams`][crate::h2::H2Connection] for ids the conn task
    /// has published via [`H2Connection::open_stream`][crate::h2::H2Connection::open_stream] that
    /// we don't yet have a [`StreamEntry`] for, and create one per id seeded with the
    /// peer-advertised initial send window and our own advertised initial recv window. No-op
    /// for server role (server streams are created by inbound HEADERS in
    /// [`finalize_new_request_stream`][super::recv]).
    fn pick_up_new_client_streams(&mut self) {
        if self.role != Role::Client {
            return;
        }
        let new_streams: Vec<(u32, Arc<StreamState>)> = {
            let shared = self.connection.streams_lock();
            shared
                .iter()
                .filter(|(id, _)| !self.streams.contains_key(*id))
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
