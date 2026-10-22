//! Conn-task → driver work-pickup boundary.
//!
//! Handler tasks queue their stream on the connection (`H2Connection::signal_driver`) as a side
//! effect of normal operation; the driver's
//! [`service_handler_signals`][H2Driver::service_handler_signals] tick takes that queue and
//! services only the streams on it, so streams waiting on their handlers cost nothing per tick.

use super::{H2Driver, Role, StreamEntry, inflow::Inflow, send::SendCursor};
use crate::{Priority, h2::transport::StreamState};
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    mem,
    sync::{Arc, atomic::Ordering},
};

impl<T> H2Driver<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    /// Service every stream that signaled since the last tick (plus newly picked-up client
    /// streams), moving each mailbox signal into driver-internal state. Three classes of signal:
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
        let mut signaled = mem::take(&mut self.signaled_scratch);
        self.connection.take_signaled_streams(&mut signaled);
        // After taking the queue, so every client stream it names is already published. New
        // streams are serviced whether or not they signaled: their parts are staged before they
        // are published, without a signal.
        self.pick_up_new_client_streams(&mut signaled);
        signaled.sort_unstable();
        signaled.dedup();

        for opaque in self.connection.drain_pending_ping_outbound() {
            self.queue_active_ping(opaque);
        }

        let mut stream_updates: Vec<(u32, u32)> = Vec::new();
        let mut connection_credit: u64 = 0;
        let mut releases: Vec<u32> = Vec::new();
        let read_target = i64::from(self.config.max_stream_recv_window_size());

        for &id in &signaled {
            // Absent: removed since it signaled.
            let Some(entry) = self.streams.get_mut(&id) else {
                continue;
            };
            // Clear before reading the mailbox, so a signal racing this pass either lands in what
            // we read or queues the stream again.
            entry.shared.needs_servicing.swap(false, Ordering::AcqRel);

            // Lazy two-tier promotion: once the handler signals it intends to read the body, grow
            // the stream window from the advertised initial to the read-target in one immediate
            // WINDOW_UPDATE. A stream whose handler never reads stays at the modest initial —
            // recv-side prioritization toward the streams the application actually cares about.
            if entry.shared.recv.is_reading.load(Ordering::Acquire) {
                let increment = entry.stream_inflow.raise_target(read_target);
                if increment > 0 {
                    stream_updates.push((id, u32::try_from(increment).unwrap_or(u32::MAX)));
                }
            }

            // Refill as the handler drains: re-grant consumed bytes up to target, batched by the
            // `Inflow` hysteresis so a chunk-by-chunk reader doesn't trigger a WINDOW_UPDATE per
            // read.
            let consumed = entry.shared.recv.bytes_consumed.swap(0, Ordering::AcqRel);
            if consumed > 0 {
                let increment = entry
                    .stream_inflow
                    .add(i64::try_from(consumed).unwrap_or(i64::MAX));
                if increment > 0 {
                    stream_updates.push((id, u32::try_from(increment).unwrap_or(u32::MAX)));
                }
                connection_credit = connection_credit.saturating_add(consumed);
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
                    if entry.send.is_none() {
                        self.sending_streams.push(id);
                    }
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
        signaled.clear();
        self.signaled_scratch = signaled;

        for (stream_id, increment) in stream_updates {
            self.queue_window_update(stream_id, increment);
        }
        if connection_credit > 0 {
            let increment = self
                .connection_inflow
                .add(i64::try_from(connection_credit).unwrap_or(i64::MAX));
            if increment > 0 {
                self.queue_window_update(0, u32::try_from(increment).unwrap_or(u32::MAX));
            }
        }
        for stream_id in releases {
            log::trace!("h2 stream {stream_id}: application released held stream — removing");
            self.remove_from_stream_maps(stream_id);
        }
    }

    /// True if any conn-task signal is pending that we haven't yet serviced: a signaled stream, a
    /// newly opened client stream, or a queued PING. Used by `park` to decide whether returning
    /// `Pending` is safe or whether we need to loop around.
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
        self.connection.has_signaled_streams() || self.connection.has_pending_ping_outbound()
    }

    /// Client role: scan [`H2Connection::streams`][crate::h2::H2Connection] for ids the conn task
    /// has published via [`H2Connection::open_stream`][crate::h2::H2Connection::open_stream] that
    /// we don't yet have a [`StreamEntry`] for, and create one per id seeded with the
    /// peer-advertised initial send window and our own advertised initial recv window, adding
    /// its id to `picked_up`. No-op for server role (server streams are created by inbound
    /// HEADERS in [`finalize_new_request_stream`][super::recv]).
    fn pick_up_new_client_streams(&mut self, picked_up: &mut Vec<u32>) {
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
        let stream_inflow = Inflow::new(i64::from(self.config.initial_stream_window_size()));
        for (id, shared) in new_streams {
            log::trace!("h2 client: driver picked up new client-opened stream {id}");
            picked_up.push(id);
            self.streams.insert(
                id,
                // Client-role streams carry no scheduling signal of their own (we don't emit
                // PRIORITY_UPDATE); the request priority a client expresses is for the *server* to
                // schedule, so the default is correct here.
                StreamEntry::new(
                    shared,
                    send_window,
                    stream_inflow,
                    None,
                    Priority::default(),
                ),
            );
        }
    }
}
