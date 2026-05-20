//! Conn-task → driver work-pickup boundary.
//!
//! Handler tasks raise the per-stream `needs_servicing` mailbox flag as a side effect of
//! normal operation; the driver's [`service_handler_signals`][H2Driver::service_handler_signals]
//! tick consults the mailbox per stream, so idle streams cost a single atomic RMW per tick.

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
    /// Scan streams for conn-task signals and turn each into driver-internal state. Five
    /// per-stream signals, all gated by the
    /// [`StreamState::needs_servicing`][StreamState] mailbox flag:
    /// - `recv.is_reading`: handler ready to read body — emit initial stream `WINDOW_UPDATE`.
    /// - `recv.bytes_consumed`: handler drained N bytes — emit stream + connection `WINDOW_UPDATE`.
    /// - `send.submission`: conn task called `submit_send` — promote into `SendCursor` for the next
    ///   outbound tick.
    /// - `pending_reset`: conn task requested a stream error (e.g. content-length guard) — emit
    ///   `RST_STREAM`, clean up.
    /// - `pending_release`: client-role wire-closed stream drop — remove from both maps without
    ///   emitting `RST_STREAM`.
    pub(super) fn service_handler_signals(&mut self) {
        // Must run before the per-stream walk so new streams' submissions are picked up
        // on the same tick.
        self.pick_up_new_client_streams();

        for opaque in self.connection.drain_pending_ping_outbound() {
            self.queue_active_ping(opaque);
        }

        // Collect into short-lived Vecs so we can act with `&mut self` after releasing
        // the streams borrow.
        let mut stream_updates: Vec<(u32, u32)> = Vec::new();
        let mut connection_credit: u64 = 0;
        let mut resets: Vec<(u32, H2ErrorCode)> = Vec::new();
        let mut releases: Vec<u32> = Vec::new();
        let max_stream_recv_window = self.config.max_stream_recv_window_size();

        for (&id, entry) in &mut self.streams {
            if !entry.shared.needs_servicing.swap(false, Ordering::AcqRel) {
                continue;
            }

            // First credit: peer hasn't been credited any recv window yet and the handler
            // signaled it's reading.
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
                    entry.send = Some(SendCursor::new(submission, &mut self.hpack_encoder));
                }
            }

            if let Some(code) = entry
                .shared
                .pending_reset
                .lock()
                .expect("pending_reset mutex poisoned")
                .take()
            {
                resets.push((id, code));
            }

            // Client-role wire-closed-but-held stream — set by `H2Transport::Drop`. No
            // RST_STREAM, wire is already closed.
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
        // Client-role guard: streams the conn task just opened are in the shared map but
        // not yet in `self.streams`. Without this, an `open_stream` landing between
        // `service_handler_signals` and `park`'s waker registration would deadlock — the
        // wake fires before the waker is registered.
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
        // Collect first to keep the shared lock short (no deadlock risk, just hygiene).
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
