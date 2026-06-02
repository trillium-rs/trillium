//! Send pump: drains each stream's [`OutboundPart`] queue into HEADERS / DATA / trailing-HEADERS /
//! `RST_STREAM` frame bytes in `H2Driver.write_buf`, and signals completion back to the conn task
//! once the submitted message is fully framed.
//!
//! Each stream's conn-task-staged parts are picked up into a driver-private [`SendCursor`] in the
//! parent's `service_handler_signals`. Per tick, [`Self::advance_one_send`] frames a stream's work:
//! an owned [`OutboundPart::Body`] is framed directly under flow control; streaming bytes a handler
//! wrote through [`H2Transport`][super::super::transport::H2Transport] flow through the per-stream
//! `outbound` ring, which is drained before any terminator. HEADERS blocks emit to `END_HEADERS`
//! without yielding to other streams (§6.10).
//!
//! All methods are on [`super::H2Driver`].

use super::{ClosedReason, DriverState, H2Driver, Role};
use crate::{
    h2::{
        H2Body, frame,
        stream_state::StreamEvent,
        transport::{OutboundPart, StreamState},
    },
    headers::hpack::{FieldSection, PseudoHeaders},
};
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    collections::VecDeque,
    io,
    pin::Pin,
    sync::{Arc, atomic::Ordering},
    task::{Context, Poll},
};

/// Driver-private state for an in-progress send on a single stream. Never observed concurrently —
/// only the driver task touches this.
///
/// The conn task's staged [`OutboundPart`]s are drained into `parts`; the driver frames them in
/// order. A [`OutboundPart::Body`] becomes the in-progress `body` (framed across ticks under flow
/// control); everything else frames atomically. Streaming bytes from an upgrade handler don't pass
/// through here — they're in the shared `outbound` ring, drained before a terminator.
#[derive(Debug, Default)]
pub(super) struct SendCursor {
    /// Parts drained from the shared `send.queue`, framed front-to-back.
    parts: VecDeque<OutboundPart>,
    /// The current owned body being framed, if any. `None` between parts.
    body: Option<H2Body>,
    /// Declared `Body::len` for the in-progress body, captured before `into_h2` consumed it. When
    /// `Some(n)` and `body_emitted == n` we transition out of the body without another `poll_read`
    /// — important when the send window is exactly at zero on the last byte.
    body_declared_len: Option<u64>,
    /// Cumulative DATA payload bytes emitted from the in-progress body.
    body_emitted: u64,
    /// `true` when the last frame attempt couldn't make progress without an external wake (body
    /// `poll_read` returned `Pending`, the ring is empty awaiting a handler write, or a window is
    /// exhausted). Read by [`H2Driver::has_pending_outbound_progress`] so the driver parks instead
    /// of busy-looping; cleared whenever a frame is emitted or framable parts remain.
    parked: bool,
}

impl SendCursor {
    /// `true` if there is no more send work staged — the queue is drained and no body is in
    /// flight. (Streaming-ring state is checked separately by the driver.)
    fn is_idle(&self) -> bool {
        self.parts.is_empty() && self.body.is_none()
    }

    /// Stage one part drained from the shared queue. A [`OutboundPart::Reset`] preempts: it
    /// abandons any parts still staged and any body mid-frame, since nothing else is valid to send
    /// once we're resetting — so a `stream_error` raised after the driver already picked up a
    /// response still resets the stream rather than framing the stale response first.
    pub(super) fn stage_part(&mut self, part: OutboundPart) {
        if matches!(part, OutboundPart::Reset(_)) {
            self.parts.clear();
            self.body = None;
            self.body_declared_len = None;
            self.body_emitted = 0;
        }
        self.parts.push_back(part);
    }

    /// The in-progress body has drained. Drop it, and if it carried trailers, splice them in as the
    /// terminator (replacing a trailing bare `Close`), so a body's `grpc-status`-style trailers
    /// frame as trailing HEADERS rather than an empty `DATA(END_STREAM)`.
    fn drain_body_into_trailers(&mut self) {
        let trailers = self.body.as_mut().and_then(H2Body::trailers);
        self.body = None;
        if let Some(trailers) = trailers {
            if matches!(self.parts.front(), Some(OutboundPart::Close)) {
                self.parts.pop_front();
            }
            self.parts.push_front(OutboundPart::Trailers(trailers));
        }
    }
}

impl<T> H2Driver<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    /// Advance every active send by at most one DATA frame per tick (HEADERS blocks emit
    /// atomically — the spec forbids interleaving HEADERS+CONTINUATION with any other frame on any
    /// stream). Body / ring reads that return `Pending` leave the cursor in place; their source
    /// wakes the driver task.
    ///
    /// No-op outside [`DriverState::Running`] / [`DriverState::Closing`]: in earlier states the
    /// preface and our initial SETTINGS haven't reached the wire. After `begin_close` queues
    /// GOAWAY, in-flight streams must still be able to emit DATA / trailers / `END_STREAM` (the
    /// spec allows frames for streams at or below the advertised last-stream-id, and late trailers
    /// from a gRPC cancellation race land after `begin_close`).
    pub(super) fn advance_outbound_sends(&mut self, cx: &mut Context<'_>) {
        if !matches!(self.state, DriverState::Running | DriverState::Closing) {
            return;
        }
        let stream_ids: Vec<u32> = self.streams.keys().copied().collect();
        for stream_id in stream_ids {
            self.advance_one_send(stream_id, cx);
        }
    }

    /// True if any stream has send work in flight — a cursor with staged parts or an in-progress
    /// body, or parts staged by the conn task that the driver hasn't picked up yet. Used to defer
    /// `Closing → Drained` until every active stream has emitted its terminator.
    pub(super) fn has_active_send_cursors(&self) -> bool {
        self.streams.values().any(|e| {
            e.send.as_ref().is_some_and(|c| !c.is_idle())
                || !e
                    .shared
                    .send
                    .queue
                    .lock()
                    .expect("send queue mutex poisoned")
                    .is_empty()
        })
    }

    /// True if any stream's recv half hasn't observed `END_STREAM` yet (peer might still send DATA
    /// or trailing HEADERS). Used by the `Closing → Drained` transition to keep the recv pump
    /// running for in-flight streams after GOAWAY.
    pub(super) fn has_pending_recv(&self) -> bool {
        self.streams
            .values()
            .any(|e| !e.shared.lifecycle_lock().recv_closed())
    }

    /// True if any stream's send half is still open — the half-closed-remote case is a server
    /// handler that has received the full request but not yet submitted its response. Such a
    /// stream has no send cursor or queued parts yet, so [`Self::has_active_send_cursors`] reads it
    /// as idle; but draining and finishing the driver here would strand the response the handler is
    /// about to submit — its `SubmitSend` would never be framed or resolved, hanging the handler
    /// task (and any graceful-shutdown guard it holds). Hold the drain until the send half closes,
    /// which happens when the handler responds (`END_STREAM`) or abandons the stream
    /// (`RST_STREAM`).
    pub(super) fn has_open_send_half(&self) -> bool {
        self.streams
            .values()
            .any(|e| !e.shared.lifecycle_lock().send_closed())
    }

    /// True if any active stream could emit a frame on the next tick without an external wake.
    ///
    /// This is the recheck `park` makes *after* registering its waker, so it must be exact:
    /// - A cursor parked on a wake it registered itself (body `poll_read` `Pending`, or a window
    ///   exhausted — resumed by the body waker or a peer `WINDOW_UPDATE`) is excluded.
    /// - A cursor with staged parts or an in-progress body has framable work.
    /// - An otherwise-idle cursor is checked against the streaming ring: bytes there (whether left
    ///   from a window-limited drain or just appended by a racing `poll_write`) are framable, and
    ///   omitting this check is exactly the missed-wake that hangs an idle bidi tunnel.
    pub(super) fn has_pending_outbound_progress(&self) -> bool {
        if self.connection_send_window <= 0 {
            return false;
        }
        self.streams.values().any(|entry| {
            let Some(cursor) = &entry.send else {
                return false;
            };
            if cursor.parked {
                return false;
            }
            if !cursor.is_idle() {
                return true;
            }
            // Only the streaming ring could still hold framable bytes.
            entry.send_window > 0
                && !entry
                    .shared
                    .send
                    .outbound
                    .lock()
                    .expect("outbound mutex poisoned")
                    .is_empty()
        })
    }

    /// Advance one stream's send cursor. Frames body bytes and ring bytes one DATA frame at a time
    /// (yielding to other streams between frames for fairness); HEADERS blocks and terminators
    /// frame in full. Drains the streaming ring before framing any terminator. Returns the cursor
    /// to the entry unless the stream completed or reset (in which case it's dropped).
    fn advance_one_send(&mut self, stream_id: u32, cx: &mut Context<'_>) {
        let Some(mut cursor) = self.streams.get_mut(&stream_id).and_then(|e| e.send.take()) else {
            return;
        };
        cursor.parked = false;

        loop {
            // Continue an in-progress body before looking at the next part.
            if cursor.body.is_some() {
                match self.poll_emit_body(stream_id, &mut cursor, cx) {
                    Poll::Ready(Ok(true)) => continue, // body drained; on to the next part
                    Poll::Ready(Ok(false)) => break,   // emitted a chunk; yield for fairness
                    Poll::Ready(Err(e)) => {
                        self.complete_and_remove_stream(stream_id, Err(e));
                        return;
                    }
                    Poll::Pending => {
                        cursor.parked = true;
                        break;
                    }
                }
            }

            let front_terminal = cursor.parts.front().is_some_and(OutboundPart::is_terminal);
            let send_open = !self.lifecycle_send_closed(stream_id);

            // Drain the streaming ring before a terminator, and while the queue is empty but the
            // send half is still open (a bidi tunnel awaiting handler writes).
            if send_open && (front_terminal || cursor.parts.is_empty()) {
                match self.poll_emit_ring(stream_id, cx) {
                    Poll::Ready(Ok(true)) => {}      // ring empty; fall through
                    Poll::Ready(Ok(false)) => break, // emitted a ring chunk; yield
                    Poll::Ready(Err(e)) => {
                        self.complete_and_remove_stream(stream_id, Err(e));
                        return;
                    }
                    Poll::Pending => {
                        cursor.parked = true;
                        break;
                    }
                }
            }

            let Some(part) = cursor.parts.pop_front() else {
                // Queue drained (and the ring is empty — checked above). Resolve the submit future
                // (END_STREAM for a normal response, the prelude handoff for a bidi upgrade). Don't
                // set `parked`: an idle bidi tunnel must stay wakeable by `poll_write`, which
                // `has_pending_outbound_progress` detects via the ring.
                self.resolve_submit_send(stream_id, Ok(()));
                break;
            };

            match part {
                OutboundPart::Headers { pseudos, headers } => {
                    // Fold END_STREAM onto the HEADERS when a bare `Close` immediately follows
                    // (a bodyless response that terminates with no DATA).
                    let end_stream = matches!(cursor.parts.front(), Some(OutboundPart::Close));
                    if end_stream {
                        cursor.parts.pop_front();
                    }
                    self.emit_headers_block(
                        stream_id,
                        &FieldSection::new(pseudos, &headers),
                        end_stream,
                    );
                    self.feed_send(stream_id, StreamEvent::SendHeaders { end_stream });
                    if end_stream {
                        self.finalize_send(stream_id);
                        return;
                    }
                }
                OutboundPart::Body(body) => {
                    cursor.body_declared_len = body.len();
                    cursor.body_emitted = 0;
                    cursor.body = Some(body.into_h2());
                }
                OutboundPart::Trailers(trailers) => {
                    self.emit_headers_block(
                        stream_id,
                        &FieldSection::new(PseudoHeaders::default(), &trailers),
                        true,
                    );
                    self.feed_send(stream_id, StreamEvent::SendHeaders { end_stream: true });
                    self.finalize_send(stream_id);
                    return;
                }
                OutboundPart::Close => {
                    self.emit_empty_end_stream(stream_id);
                    self.feed_send(stream_id, StreamEvent::SendData { end_stream: true });
                    self.finalize_send(stream_id);
                    return;
                }
                OutboundPart::Reset(code) => {
                    log::debug!("h2 stream {stream_id}: conn-task-requested RST_STREAM({code:?})");
                    // `queue_rst_stream` now feeds the terminal `SendReset` to the lifecycle
                    // itself.
                    self.queue_rst_stream(stream_id, code);
                    self.complete_and_remove_stream(
                        stream_id,
                        Err(io::Error::other(format!(
                            "stream reset requested: {code:?}"
                        ))),
                    );
                    return;
                }
            }
        }

        if let Some(entry) = self.streams.get_mut(&stream_id) {
            entry.send = Some(cursor);
        }
    }

    /// `true` if the stream's send half is closed (or the stream is gone).
    fn lifecycle_send_closed(&self, stream_id: u32) -> bool {
        self.streams
            .get(&stream_id)
            .is_none_or(|e| e.shared.lifecycle_lock().send_closed())
    }

    /// Feed a send-side event to the stream's lifecycle. Send transitions are driver-controlled and
    /// always legal if the pump respects the machine; an illegal one is `debug_assert`ed and
    /// absorbed inside `on_event`, so the result is intentionally ignored.
    fn feed_send(&self, stream_id: u32, event: StreamEvent) {
        if let Some(entry) = self.streams.get(&stream_id) {
            let _ = entry.shared.apply_event(event);
        }
    }

    /// Poll the in-progress body for one DATA chunk, bounded by per-stream and connection send
    /// windows. `Ready(Ok(true))` means the body drained (cursor advanced to the next part, any
    /// body trailers spliced in as the terminator); `Ready(Ok(false))` means a DATA frame was
    /// emitted; `Pending` means no bytes or no window right now.
    fn poll_emit_body(
        &mut self,
        stream_id: u32,
        cursor: &mut SendCursor,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<bool>> {
        // Fast path: declared length reached — transition without another poll. Lets us close out
        // a stream whose window just barely sufficed without waiting on a superfluous
        // WINDOW_UPDATE.
        if cursor.body_declared_len == Some(cursor.body_emitted) {
            cursor.drain_body_into_trailers();
            return Poll::Ready(Ok(true));
        }

        let stream_window = self.streams.get(&stream_id).map_or(0, |e| e.send_window);
        let budget = stream_window.min(self.connection_send_window);
        if budget <= 0 {
            return Poll::Pending; // peer WINDOW_UPDATE on the read path will wake us
        }
        let cap = usize::try_from(budget)
            .unwrap_or(usize::MAX)
            .min(self.body_scratch.len());

        // Single-copy framing: read the body straight into the tail of `write_buf` behind a
        // reserved DATA-header prefix, then backfill the header. `write_buf` is
        // driver-exclusive, so this needs no lock and no `unsafe`. The zero-init that
        // `resize` performs is what makes the `poll_read` slice safe to hand out; it costs
        // one memset in place of the eliminated scratch→`write_buf` memcpy. (The streaming
        // ring in `poll_emit_ring` keeps `body_scratch` — its bytes come from another task.)
        //
        // Every early return below must `truncate(header_pos)` to undo this reservation —
        // otherwise the zero-filled tail leaks onto the wire as a malformed DATA payload.
        let prefix_len = frame::data::encoded_prefix_len(0);
        let header_pos = self.write_buf.len();
        let payload_start = header_pos + prefix_len;
        self.write_buf.resize(payload_start + cap, 0);

        let body = cursor.body.as_mut().expect("caller checked body.is_some()");
        let n = match Pin::new(body).poll_read(cx, &mut self.write_buf[payload_start..]) {
            Poll::Ready(Ok(n)) => n,
            Poll::Ready(Err(e)) => {
                self.write_buf.truncate(header_pos);
                return Poll::Ready(Err(e));
            }
            Poll::Pending => {
                self.write_buf.truncate(header_pos);
                return Poll::Pending;
            }
        };
        if n == 0 {
            self.write_buf.truncate(header_pos);
            cursor.drain_body_into_trailers();
            return Poll::Ready(Ok(true));
        }

        let n_u32 = u32::try_from(n).expect("read n <= body_scratch.len() fits u32");
        log::trace!("h2 emit: DATA stream={stream_id} len={n} end_stream=false");
        // Backfill the 9-byte DATA header in front of the payload, then drop the unused
        // tail of the reservation (a short read gives `n < cap`).
        frame::data::encode_prefix(
            stream_id,
            // Never END_STREAM here; trailers / empty-DATA carries END_STREAM.
            false,
            n_u32,
            0,
            &mut self.write_buf[header_pos..payload_start],
        )
        .expect("prefix slice sized from encoded_prefix_len");
        self.write_buf.truncate(payload_start + n);
        self.write_flush_pending = true;

        let charge = i64::try_from(n).expect("n <= body_scratch.len() fits i64");
        self.connection_send_window -= charge;
        if let Some(entry) = self.streams.get_mut(&stream_id) {
            entry.send_window -= charge;
        }
        cursor.body_emitted += n as u64;

        // If the declared length is now satisfied, transition within this call so the loop frames
        // the terminator instead of parking for a WINDOW_UPDATE that won't come.
        if cursor.body_declared_len == Some(cursor.body_emitted) {
            cursor.drain_body_into_trailers();
            return Poll::Ready(Ok(true));
        }
        Poll::Ready(Ok(false))
    }

    /// Drain one DATA frame's worth of bytes from the streaming `outbound` ring, bounded by the
    /// send windows. `Ready(Ok(true))` means the ring is empty; `Ready(Ok(false))` means a frame
    /// was emitted; `Pending` means bytes remain but no window.
    fn poll_emit_ring(&mut self, stream_id: u32, _cx: &mut Context<'_>) -> Poll<io::Result<bool>> {
        let Some(shared) = self.streams.get(&stream_id).map(|e| Arc::clone(&e.shared)) else {
            return Poll::Ready(Ok(true));
        };
        let ring_len = shared
            .send
            .outbound
            .lock()
            .expect("outbound mutex poisoned")
            .len();
        if ring_len == 0 {
            return Poll::Ready(Ok(true));
        }

        let stream_window = self.streams.get(&stream_id).map_or(0, |e| e.send_window);
        let budget = stream_window.min(self.connection_send_window);
        if budget <= 0 {
            return Poll::Pending;
        }
        let cap = usize::try_from(budget)
            .unwrap_or(usize::MAX)
            .min(self.body_scratch.len())
            .min(ring_len);

        {
            let mut ring = shared
                .send
                .outbound
                .lock()
                .expect("outbound mutex poisoned");
            self.body_scratch[..cap].copy_from_slice(&ring[..cap]);
            ring.ignore_front(cap);
        }
        // Drained bytes freed ring capacity — wake any writer parked on backpressure.
        shared.send.outbound_write_waker.wake();

        let cap_u32 = u32::try_from(cap).expect("cap <= body_scratch.len() fits u32");
        log::trace!("h2 emit: DATA stream={stream_id} len={cap} end_stream=false (ring)");
        self.queue_frame(frame::data::encoded_prefix_len(0), |buf| {
            frame::data::encode_prefix(stream_id, false, cap_u32, 0, buf)
        });
        self.write_buf.extend_from_slice(&self.body_scratch[..cap]);

        let charge = i64::try_from(cap).expect("cap fits i64");
        self.connection_send_window -= charge;
        if let Some(entry) = self.streams.get_mut(&stream_id) {
            entry.send_window -= charge;
        }
        Poll::Ready(Ok(false))
    }

    /// Emit a HEADERS block (initial response/request or trailers) as HEADERS + CONTINUATION
    /// fragments up to `END_HEADERS`, encoding against the connection HPACK encoder at frame time
    /// so the wire order matches the dynamic-table mutation order. `end_stream` is set on the first
    /// fragment.
    fn emit_headers_block(
        &mut self,
        stream_id: u32,
        field_section: &FieldSection<'_>,
        end_stream: bool,
    ) {
        // Reuse the retained scratch (take/restore so the chunking loop below can still borrow
        // `self` for queue_frame / write_buf). After the first response grows it, the encode is
        // allocation-free.
        let mut encoded = std::mem::take(&mut self.headers_scratch);
        encoded.clear();
        self.hpack_encoder.encode(field_section, &mut encoded);
        let max_payload = self
            .connection
            .current_peer_settings()
            .effective_max_frame_size() as usize;

        let mut offset = 0;
        let mut first = true;
        loop {
            let remaining = encoded.len() - offset;
            let chunk = remaining.min(max_payload);
            let end_headers = chunk == remaining;
            let chunk_u32 = u32::try_from(chunk).expect("chunk <= max_frame_size fits u32");
            if first {
                log::trace!(
                    "h2 emit: HEADERS stream={stream_id} len={chunk} end_headers={end_headers} \
                     end_stream={end_stream}",
                );
                self.queue_frame(frame::headers::encoded_prefix_len(0, false), |buf| {
                    frame::headers::encode_prefix(
                        stream_id,
                        end_stream,
                        end_headers,
                        None,
                        chunk_u32,
                        0,
                        buf,
                    )
                });
            } else {
                log::trace!(
                    "h2 emit: CONTINUATION stream={stream_id} len={chunk} \
                     end_headers={end_headers}",
                );
                self.queue_frame(frame::continuation::ENCODED_PREFIX_LEN, |buf| {
                    frame::continuation::encode_prefix(stream_id, end_headers, chunk_u32, buf)
                });
            }
            self.write_buf
                .extend_from_slice(&encoded[offset..offset + chunk]);
            offset += chunk;
            first = false;
            if end_headers {
                break;
            }
        }

        // Restore the (now grown) buffer for the next header block to reuse.
        self.headers_scratch = encoded;
    }

    /// Emit an empty `DATA(END_STREAM)` frame as the stream terminator.
    fn emit_empty_end_stream(&mut self, stream_id: u32) {
        log::trace!("h2 emit: DATA stream={stream_id} len=0 end_stream=true (terminator)");
        self.queue_frame(frame::data::encoded_prefix_len(0), |buf| {
            frame::data::encode_prefix(stream_id, true, 0, 0, buf)
        });
    }

    /// Send pump's success-path completion: the terminator has been framed and the lifecycle
    /// transitioned (the terminator's event was fed before this call). Resolves the conn task's
    /// `SubmitSend`, then — if the stream is now fully `Closed` — tears it down: the server
    /// removes it; the client keeps the entry in the map for post-EOF trailer access and
    /// removes it on transport drop. If the recv half is still open (`HalfClosedLocal`) the
    /// stream lingers until the peer's `END_STREAM` lands via [`route_data`][super::recv].
    pub(super) fn finalize_send(&mut self, stream_id: u32) {
        self.resolve_submit_send(stream_id, Ok(()));
        self.close_if_both_done(stream_id);
    }

    /// Close and tear down the stream if both halves are done (`lifecycle.is_closed()`). Server
    /// removes the entry; client keeps it (for trailer access) and signals close. No-op
    /// otherwise. Called from both the send terminator path and the recv `END_STREAM` path.
    pub(super) fn close_if_both_done(&mut self, stream_id: u32) {
        let closed = self
            .streams
            .get(&stream_id)
            .is_some_and(|e| e.shared.lifecycle_lock().is_closed());
        if !closed {
            return;
        }
        match self.role {
            Role::Server => self.complete_and_remove_stream(stream_id, Ok(())),
            Role::Client => self.signal_close(stream_id, Ok(())),
        }
    }

    /// Signal send completion + record the close reason, but keep the stream in both maps. Used by
    /// client-role clean completion (the application's [`H2Transport`] keeps a working handle for
    /// trailer access; removal happens on transport drop) and as the inner half of
    /// [`Self::complete_and_remove_stream`].
    pub(super) fn signal_close(&mut self, stream_id: u32, result: io::Result<()>) {
        log::trace!("h2 stream {stream_id}: completing send ({result:?})");
        let reason = if result.is_err() {
            ClosedReason::Reset
        } else {
            ClosedReason::EndStream
        };
        self.closed_streams.record(stream_id, reason);
        if let Some(entry) = self.streams.get(&stream_id) {
            resolve_submit_send(&entry.shared, result);
            // Wake every conn-task-side waiter so a handler parked on this stream observes the
            // teardown instead of hanging (and leaking its swansong guard). A stream the driver
            // tears down on its own — stream-level RST, flow-control overflow, malformed trailers —
            // strands a handler parked reading the request body (`recv.waker`) or writing a bidi
            // response (`outbound_write_waker`) unless we wake them here; the FSM is already
            // recv/send-closed, so they re-poll to EOF / `BrokenPipe`. `response_headers_waker` is
            // the client-role analog (no-op on server streams).
            entry.shared.recv.waker.wake();
            entry.shared.recv.response_headers_waker.wake();
            entry.shared.send.outbound_write_waker.wake();
        }
    }

    /// Resolve the conn task's `SubmitSend` future (idempotent) without touching the maps.
    fn resolve_submit_send(&self, stream_id: u32, result: io::Result<()>) {
        if let Some(entry) = self.streams.get(&stream_id) {
            resolve_submit_send(&entry.shared, result);
        }
    }

    /// Signal completion, then remove the stream from the driver's private map and the connection's
    /// shared map. Used by error / reset paths and server-role clean completion.
    pub(super) fn complete_and_remove_stream(&mut self, stream_id: u32, result: io::Result<()>) {
        self.signal_close(stream_id, result);
        self.remove_from_stream_maps(stream_id);
    }

    /// Drop the entry from the driver's private map and the connection's shared map.
    pub(super) fn remove_from_stream_maps(&mut self, stream_id: u32) {
        self.streams.remove(&stream_id);
        self.connection.streams_lock().remove(&stream_id);
    }
}

/// Resolve the conn task's [`SubmitSend`][super::super::SubmitSend]: store the result, flip
/// `submit_resolved`, wake its waker. Idempotent — a no-op once already resolved (the upgrade path
/// resolves early at the prelude handoff, so the eventual terminator must not clobber the result).
fn resolve_submit_send(state: &StreamState, result: io::Result<()>) {
    if state.send.submit_resolved.load(Ordering::Acquire) {
        return;
    }
    *state
        .send
        .completion_result
        .lock()
        .expect("completion_result mutex poisoned") = Some(result);
    state.send.submit_resolved.store(true, Ordering::Release);
    state.send.completion_waker.wake();
}
