//! Send pump: turns conn-task-submitted responses ([`SendCursor`]s) into HEADERS / DATA /
//! trailing-HEADERS frame bytes in `H2Driver.write_buf`, and signals completion back to the
//! conn task once the response is fully on the wire.
//!
//! Picks up new submissions from per-stream `StreamState.send.submission` slots in the
//! parent's `service_handler_signals`. Per-tick, advances each active send by one frame
//! (with the HEADERS+CONTINUATION exception: that pair runs to `END_HEADERS` without
//! yielding to other streams, per the spec).
//!
//! All methods are on [`super::H2Driver`].

use super::{ClosedReason, DriverState, H2Driver, Role};
use crate::{
    Body, Headers,
    h2::{
        H2Body, frame,
        lifecycle::Submission,
        transport::{H2OutboundReader, StreamState},
    },
    headers::hpack::{FieldSection, HpackEncoder, PseudoHeaders},
};
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    io,
    pin::Pin,
    sync::{Arc, atomic::Ordering},
    task::{Context, Poll},
};

/// Driver-private state for an in-progress response on a single stream. Never observed
/// concurrently — only the driver task touches this.
#[derive(Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "driver-private framing flags, each tracking an independent step of the send \
              sequence — not a cross-task state set (that's StreamLifecycle)"
)]
pub(super) struct SendCursor {
    /// Materialized HEADERS bytes — produced at submission pickup by committing the
    /// conn-task-built [`crate::headers::hpack::EncodedBlock`] under the encoder's write
    /// lock. Chunked into HEADERS + CONTINUATION frames as `peer_max_frame_size` allows.
    encoded_headers: Vec<u8>,
    /// Offset into `encoded_headers` of the first byte not yet emitted.
    headers_offset: usize,
    /// Whether this stream's response carries a body. Decides whether the final HEADERS
    /// fragment carries `END_STREAM` (no body, no trailers) or whether we transition to
    /// the Body phase next.
    has_body: bool,
    /// Body source, wrapped in [`H2Body`] so its `AsyncRead` yields plain payload bytes
    /// (no h1 chunked-encoding wrapping) suitable for DATA frame payloads. Driver polls
    /// `body.poll_read` to fill DATA frames; transitions to None once drained (a 0-byte
    /// read) or once `body_bytes_emitted == body_len`.
    body: Option<H2Body>,
    /// Declared `Body::len` at cursor creation, if known. When `Some(n)` and
    /// `body_bytes_emitted == n`, we can transition out of the Body phase without another
    /// `body.poll_read` — important when send flow-control windows are exactly at zero on
    /// the last byte: without this, we'd wait for a superfluous `WINDOW_UPDATE` before
    /// detecting EOF.
    body_len: Option<u64>,
    /// Cumulative DATA payload bytes emitted from the body.
    body_bytes_emitted: u64,
    /// Trailers, populated from `body.trailers()` once the body is fully drained, or by
    /// [`H2Connection::submit_trailers`][crate::h2::H2Connection::submit_trailers].
    pub(super) trailers: Option<Headers>,
    /// Where we are in the response.
    phase: SendPhase,
    /// `true` if this stream is an upgrade (extended-CONNECT RFC 8441, or a raw
    /// `Conn::upgrade`): HEADERS goes out without `END_STREAM`, the cursor frames the
    /// prelude body (if any), and on prelude drain it signals
    /// [`SubmitSend`][super::super::SubmitSend] completion and switches to sourcing DATA
    /// from [`SendState::outbound`][super::super::transport::SendState] (the upgrade
    /// handler's post-handoff writes) — see [`Self::continuation_started`].
    is_upgrade: bool,
    /// Upgrade only: `false` while framing the prelude body, flipped `true` at prelude
    /// drain when the cursor swaps to the `H2OutboundReader` continuation source. Gates
    /// the one-time "signal completion + swap body" step so a draining continuation
    /// terminates the stream instead of swapping again.
    continuation_started: bool,
    /// `true` once `signal_submit_resolved` has been called for this cursor — prevents the
    /// upgrade-early-completion path and the eventual `complete_and_remove_stream` call from
    /// double-signaling the conn task's `SubmitSend` future.
    completion_signaled: bool,
}

impl SendCursor {
    /// Materialize the conn-task-built HEADERS plan into wire bytes by committing it
    /// against `encoder` (write lock acquired internally), then assemble the cursor.
    ///
    /// Commits happen in submission-pickup order on the driver task; the same iteration
    /// order is used by [`H2Driver::advance_outbound_sends`] to emit HEADERS frames into
    /// `write_buf`, so the wire order matches the dynamic-table mutation order — required
    /// by HPACK's stateful decoder.
    pub(super) fn new(submission: Submission, encoder: &mut HpackEncoder) -> Self {
        // An upgrade always keeps the stream open (HEADERS without END_STREAM) even with no
        // prelude body — the post-upgrade continuation arrives via the outbound queue, which
        // the pump sources lazily once the prelude (if any) drains.
        let has_body = submission.is_upgrade || submission.body.is_some();
        // Capture `Body::len` before the `into_h2()` consumes it — H2Body intentionally
        // doesn't expose the inner length (the send pump uses it for the early-EOF
        // optimization in `poll_emit_one_data`).
        let body_len = submission.body.as_ref().and_then(Body::len);
        // Encode HEADERS synchronously on the driver task against the live dynamic-table
        // state. Submissions are picked up in order so the wire-emission order matches
        // the dyn-table mutation order — required by HPACK's stateful decoder.
        let mut encoded_headers = Vec::new();
        encoder.encode(&submission.field_section(), &mut encoded_headers);
        Self {
            encoded_headers,
            headers_offset: 0,
            has_body,
            body: submission.body.map(Body::into_h2),
            body_len,
            body_bytes_emitted: 0,
            trailers: None,
            phase: SendPhase::Headers,
            is_upgrade: submission.is_upgrade,
            continuation_started: false,
            completion_signaled: false,
        }
    }
}

/// Position of a `SendCursor` in the response lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendPhase {
    /// Still emitting HEADERS + CONTINUATION fragments.
    Headers,
    /// HEADERS done; pumping body bytes into DATA frames. `parked` is `true` when the last
    /// body `poll_read` returned `Pending` — the body registered its own waker (an
    /// extended-CONNECT upgrade reader waiting on the handler to write, or a genuinely async
    /// response body), so an external wake is what advances us, not another driver tick.
    /// Read by [`has_pending_outbound_progress`][H2Driver::has_pending_outbound_progress] so
    /// a parked body doesn't keep the driver awake busy-looping. Reset to `false` whenever a
    /// poll yields bytes.
    Body { parked: bool },
    /// Body fully drained; emit trailing HEADERS (if trailers) or empty `DATA(END_STREAM)`.
    Trailers,
    /// Completion has been signaled to the conn task; entry can be cleaned up.
    Complete,
}

impl<T> H2Driver<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    /// Advance every active send by at most one step per tick (headers fragments are
    /// emitted atomically per stream — the spec forbids interleaving HEADERS+CONTINUATION
    /// with any other frame on any other stream). Body reads that return Pending leave the
    /// cursor in place; the body's source will wake the driver task when bytes are
    /// available.
    ///
    /// No-op outside [`DriverState::Running`]: in earlier states the connection preface
    /// and our initial SETTINGS haven't reached the wire yet, and emitting HEADERS before
    /// them would violate the spec. Server-side this is moot (no streams exist
    /// pre-Running); client-side it matters because `H2Connection::open_stream` can stage
    /// a submission any time after the connection is created.
    pub(super) fn advance_outbound_sends(&mut self, cx: &mut Context<'_>) {
        // Run in both Running and Closing. After `begin_close` queues GOAWAY, in-flight
        // streams must still be able to emit DATA / trailing HEADERS / END_STREAM —
        // the spec allows frames after GOAWAY for streams whose ids are at or below the
        // `last-stream-id` we advertised, and gRPC handlers (etc.) typically submit
        // trailers *after* their cancellation race has resolved, which is *after*
        // begin_close has already fired. Without this, late trailers stay parked in
        // `pending_trailers` forever and the peer hangs waiting for them.
        if !matches!(self.state, DriverState::Running | DriverState::Closing) {
            return;
        }
        let stream_ids: Vec<u32> = self.streams.keys().copied().collect();
        for stream_id in stream_ids {
            self.advance_one_send(stream_id, cx);
        }
    }

    /// True if any stream has an in-flight send — either a `SendCursor` already built on
    /// the driver side (cursor exists, framing in progress) OR a submission staged by the
    /// conn task that the driver hasn't yet picked up. Used to defer the
    /// `Closing → Drained` transition: we must keep ticking the send pump until every
    /// active stream has emitted its trailers / `END_STREAM`.
    pub(super) fn has_active_send_cursors(&self) -> bool {
        self.streams
            .values()
            .any(|e| e.send.is_some() || e.shared.lifecycle_lock().has_active_send())
    }

    /// True if any stream's recv half hasn't observed `END_STREAM` yet (peer might
    /// still send DATA or trailing HEADERS). Used by the `Closing → Drained` transition
    /// so we keep the recv pump running for in-flight streams after our (or the peer's)
    /// GOAWAY.
    pub(super) fn has_pending_recv(&self) -> bool {
        self.streams
            .values()
            .any(|e| e.shared.lifecycle_lock().has_pending_recv())
    }

    /// True if any active stream has more outbound work that could make progress on the next
    /// tick — a `SendCursor` mid-Headers / Trailers / Complete (no flow control gates these),
    /// or a `SendCursor` in Body that returned bytes on its last poll, has a positive
    /// per-stream send window, AND a positive connection send window. Used by
    /// [`park`][super::H2Driver::park] to keep the driver awake when there are body bytes
    /// left to emit and budget to emit them with — the body source wouldn't wake us in that
    /// case (it already returned `Ready` on the prior poll), and the only frame the peer is
    /// obliged to send is a `WINDOW_UPDATE` once our budget runs out, not before.
    ///
    /// A body whose last poll returned `Pending` (`Body { parked: true }`) is excluded: it
    /// registered its own waker, so an external wake — a handler write on an extended-CONNECT
    /// upgrade stream, or an async body becoming readable — is what resumes us. Counting it
    /// here would defeat `park` and busy-spin the driver.
    pub(super) fn has_pending_outbound_progress(&self) -> bool {
        if self.connection_send_window <= 0 {
            return false;
        }
        self.streams.values().any(|entry| match &entry.send {
            None => false,
            Some(send) => match send.phase {
                SendPhase::Headers | SendPhase::Trailers | SendPhase::Complete => true,
                SendPhase::Body { parked } => entry.send_window > 0 && !parked,
            },
        })
    }

    /// Advance one stream's `SendCursor` by one frame's worth of work, with the
    /// HEADERS+CONTINUATION exception: in `Headers` phase we keep emitting fragments
    /// back-to-back until `END_HEADERS` is set. Other phases emit at most one frame per
    /// tick to keep streams roughly fair.
    fn advance_one_send(&mut self, stream_id: u32, cx: &mut Context<'_>) {
        let Some(mut send) = self.streams.get_mut(&stream_id).and_then(|e| e.send.take()) else {
            return;
        };

        loop {
            match send.phase {
                SendPhase::Headers => {
                    // Spec forbids interleaving HEADERS+CONTINUATION with any other frame,
                    // including frames on other streams. The unconditional loop iteration
                    // that follows keeps emitting fragments while still in Headers, or
                    // moves into the new phase this tick if transitioned (avoiding an
                    // extra park cycle).
                    self.emit_one_headers_fragment(stream_id, &mut send);
                }
                SendPhase::Body { .. } => match self.poll_emit_one_data(stream_id, &mut send, cx) {
                    Poll::Ready(Ok(())) => {
                        // Body returned Ready(N>0) (emitted DATA, still Body) or Ready(0)
                        // (transitioned to Trailers). On transition, run the new phase
                        // this tick; on stay-in-Body, yield to the next stream.
                        if matches!(send.phase, SendPhase::Body { .. }) {
                            break;
                        }
                    }
                    Poll::Ready(Err(e)) => {
                        self.complete_and_remove_stream(stream_id, Err(e));
                        return;
                    }
                    Poll::Pending => break, // body's source will wake the driver task
                },
                SendPhase::Trailers => {
                    // Always transitions to Complete; the next loop iteration fires it.
                    self.emit_trailers_or_end_stream(stream_id, &mut send);
                }
                SendPhase::Complete => {
                    self.finalize_send(stream_id);
                    return;
                }
            }
        }

        // Cursor still active — put it back.
        if let Some(entry) = self.streams.get_mut(&stream_id) {
            entry.send = Some(send);
        }
    }

    /// Signal send completion on the stream's `StreamState`, then remove the stream from
    /// both the driver's private map and `H2Connection.streams`. After this the conn
    /// task's pending `SubmitSend` future will see `completed = true` on its next poll
    /// and resolve.
    ///
    /// Records the close reason in the driver's closed-stream ledger so that any late
    /// peer frames on this stream get the correct error category: an `Err` result —
    /// which always follows a `queue_rst_stream` call in the error paths — records as
    /// `Reset`, and an `Ok` result (clean `END_STREAM` completion from the send pump)
    /// records as `EndStream`.
    ///
    /// On the extended-CONNECT upgrade path, completion is signaled early (right after
    /// HEADERS go out — see [`emit_one_headers_fragment`][Self::emit_one_headers_fragment]),
    /// so by the time we reach this teardown the conn task has long since returned from
    /// `send_h2` and started `handler.upgrade(...)`. `entry.send.completion_signaled`
    /// gates re-signaling here to avoid clobbering the result the conn task already saw.
    pub(super) fn complete_and_remove_stream(&mut self, stream_id: u32, result: io::Result<()>) {
        self.signal_close(stream_id, result);
        self.remove_from_stream_maps(stream_id);
    }

    /// Wire-close half of [`Self::complete_and_remove_stream`]: record in the closed-streams
    /// ledger, signal the conn task's pending [`SubmitSend`][super::super::SubmitSend], wake
    /// any task parked on response headers. Does **not** remove the stream from either map.
    ///
    /// Used directly by client-role clean completion ([`Self::try_close_if_both_done`]),
    /// which intentionally keeps the stream in the map so the application's
    /// [`H2Transport`][super::super::H2Transport] still has a working handle for trailer
    /// access etc. Map removal happens later via the application dropping its transport,
    /// which signals through `pending_release` (handled by `service_handler_signals`).
    pub(super) fn signal_close(&mut self, stream_id: u32, result: io::Result<()>) {
        log::trace!("h2 stream {stream_id}: completing send ({result:?})");
        let reason = if result.is_err() {
            ClosedReason::Reset
        } else {
            ClosedReason::EndStream
        };
        self.closed_streams.record(stream_id, reason);
        if let Some(entry) = self.streams.get(&stream_id) {
            let already_signaled = entry.send.as_ref().is_some_and(|c| c.completion_signaled);
            if already_signaled {
                log::trace!(
                    "h2 stream {stream_id}: skipping signal_submit_resolved (already resolved by \
                     upgrade path)"
                );
            } else {
                signal_submit_resolved(&entry.shared, result);
            }
            // Wake any conn task parked on `H2Connection::response_headers` — the slot
            // is empty (we never stashed for this id, otherwise the take would have already
            // happened normally), so the wake makes the parked poll re-check the streams map,
            // find the id absent, and surface `NotConnected`. Idempotent / no-op on
            // server-role streams (the slot is never written there).
            entry.shared.recv.response_headers_waker.wake();
        }
    }

    /// Map-removal half of [`Self::complete_and_remove_stream`]: drop the entry from the
    /// driver's private map and the connection's shared map. Called immediately by error /
    /// server-role completion paths, and on application-side release for client-role
    /// wire-closed-but-held streams.
    pub(super) fn remove_from_stream_maps(&mut self, stream_id: u32) {
        self.streams.remove(&stream_id);
        self.connection.streams_lock().remove(&stream_id);
    }

    /// Send pump's success-path completion: the `END_STREAM` terminator has been framed.
    /// Resolves the conn task's pending [`SubmitSend`][super::super::SubmitSend] (a no-op
    /// on the upgrade path, which resolved it early at handoff), then records the send
    /// half wire-closed via [`StreamLifecycle::mark_send_closed`] — the lifecycle's
    /// `LocalClosed` is the canonical "send done" fact the close paths read.
    ///
    /// For **server** role this removes the stream once the peer's `END_STREAM` has also
    /// arrived; for **client** role it keeps the entry (for trailer access) and just
    /// signals close. Either way, if the recv side is still open the stream stays in
    /// `LocalClosed { recv_eof: false }` until the peer's `END_STREAM` lands (via
    /// [`route_data`][super::recv] or the HEADERS-with-`END_STREAM` case in
    /// [`finalize_response_headers`][super::recv]), or an error/RST routes through
    /// [`Self::complete_and_remove_stream`] directly.
    pub(super) fn finalize_send(&mut self, stream_id: u32) {
        if let Some(entry) = self.streams.get_mut(&stream_id) {
            let already_signaled = entry.send.as_ref().is_some_and(|c| c.completion_signaled);
            if !already_signaled {
                signal_submit_resolved(&entry.shared, Ok(()));
                if let Some(c) = entry.send.as_mut() {
                    c.completion_signaled = true;
                }
            }
            entry.shared.lifecycle_lock().mark_send_closed();
        }
        match self.role {
            Role::Server => self.close_server_stream_if_both_done(stream_id),
            Role::Client => self.try_close_if_both_done(stream_id),
        }
    }

    /// Wire-close the stream if both halves have completed. Used by the client-role
    /// lifecycle — the recv-side `END_STREAM` path in [`route_data`][super::recv], the
    /// HEADERS-with-`END_STREAM` case in
    /// [`finalize_response_headers`][super::recv], and [`Self::finalize_send`] for the
    /// client branch.
    ///
    /// Performs the wire-close work (closed-streams ledger + send-completion signaling +
    /// response-headers waker fire) but **keeps the entry in both stream maps** so the
    /// application's [`H2Transport`][super::super::H2Transport] retains a working handle
    /// for response-trailer access, etc.; the stream lives until the application drops
    /// its conn. Map removal happens via `pending_release` triggered by `H2Transport::Drop`
    /// and serviced in `service_handler_signals`.
    ///
    /// No-op if either side is still in flight.
    pub(super) fn try_close_if_both_done(&mut self, stream_id: u32) {
        let Some(entry) = self.streams.get(&stream_id) else {
            return;
        };
        if entry.shared.lifecycle_lock().is_fully_closed() {
            self.signal_close(stream_id, Ok(()));
        }
    }

    /// Server-role: fully close and remove the stream once *both* halves are done.
    ///
    /// Sending our complete response (`END_STREAM`) only moves the stream to half-closed
    /// (local) — RFC 9113 §5.1 keeps it open for receiving until the peer also half-closes.
    /// Tearing it down on send completion alone makes the peer's subsequent (legal)
    /// `END_STREAM` look like a frame on a closed stream, which earns a spurious
    /// `RST_STREAM(STREAM_CLOSED)` that races back and destroys trailers we already sent.
    /// So removal waits until the peer's `END_STREAM` has arrived too. Driven from both ends:
    /// [`finalize_send`][Self::finalize_send] when send completes, and
    /// [`route_data`][super::recv] when the peer's `END_STREAM` lands.
    ///
    /// Unlike the client-role [`try_close_if_both_done`][Self::try_close_if_both_done],
    /// which keeps the entry in the map for the application's trailer access, the server
    /// has no handle left once the response is sent, so this removes the stream outright.
    ///
    /// No-op if either side is still in flight.
    pub(super) fn close_server_stream_if_both_done(&mut self, stream_id: u32) {
        let Some(entry) = self.streams.get(&stream_id) else {
            return;
        };
        if entry.shared.lifecycle_lock().is_fully_closed() {
            self.complete_and_remove_stream(stream_id, Ok(()));
        }
    }

    /// Emit one HEADERS or CONTINUATION fragment from `send.encoded_headers`. Transitions
    /// `send.phase` to `Body` / `Trailers` / `Complete` once `END_HEADERS` is set. The
    /// first fragment is HEADERS; subsequent fragments (when `headers_offset > 0`) are
    /// CONTINUATION.
    fn emit_one_headers_fragment(&mut self, stream_id: u32, send: &mut SendCursor) {
        let max_payload = self
            .connection
            .current_peer_settings()
            .effective_max_frame_size() as usize;
        let remaining = send.encoded_headers.len() - send.headers_offset;
        let chunk_len = remaining.min(max_payload);
        let end_headers = chunk_len == remaining;
        let is_first = send.headers_offset == 0;
        let chunk_len_u32 =
            u32::try_from(chunk_len).expect("chunk_len <= effective_max_frame_size fits u32");

        if is_first {
            // Final HEADERS fragment with no body and no trailers carries END_STREAM.
            let end_stream = end_headers && !send.has_body;
            log::trace!(
                "h2 emit: HEADERS stream={stream_id} len={chunk_len} end_headers={end_headers} \
                 end_stream={end_stream}",
            );
            self.queue_frame(frame::headers::encoded_prefix_len(0, false), |buf| {
                frame::headers::encode_prefix(
                    stream_id,
                    end_stream,
                    end_headers,
                    None,
                    chunk_len_u32,
                    0,
                    buf,
                )
            });
        } else {
            log::trace!(
                "h2 emit: CONTINUATION stream={stream_id} len={chunk_len} \
                 end_headers={end_headers}",
            );
            self.queue_frame(frame::continuation::ENCODED_PREFIX_LEN, |buf| {
                frame::continuation::encode_prefix(stream_id, end_headers, chunk_len_u32, buf)
            });
        }

        // Append the header-block fragment payload.
        self.write_buf.extend_from_slice(
            &send.encoded_headers[send.headers_offset..send.headers_offset + chunk_len],
        );
        send.headers_offset += chunk_len;

        if end_headers {
            // For an upgrade, completion is signaled later — when the prelude body drains
            // and the cursor swaps to the outbound continuation (see `end_of_body`) — so
            // control returns to the conn task only after the prelude is on the wire,
            // matching h1/h3.
            send.phase = if send.has_body {
                SendPhase::Body { parked: false }
            } else if is_first {
                // Single HEADERS carried END_STREAM; nothing more to emit.
                SendPhase::Complete
            } else {
                // Multi-fragment + no-body case: END_STREAM was not set on the first
                // HEADERS (because end_headers was false then), and CONTINUATION has no
                // END_STREAM flag. Transition to Trailers so the next tick emits an
                // empty DATA(END_STREAM) as the stream terminator. Rare in practice —
                // response headers usually fit in one peer-default 16 KiB frame — but
                // spec-correct when a response has lots of large headers.
                SendPhase::Trailers
            };
        }
    }

    /// Poll the body for one DATA chunk, respecting both per-stream and connection send
    /// flow-control windows. On `Ready(Ok(0))`, takes trailers off the
    /// body and transitions to `Trailers`. On `Ready(Ok(n))`, emits one DATA frame (no
    /// `END_STREAM`) and decrements both windows by `n`. On `Pending`, the cursor stays in
    /// `Body`:
    /// - If the cause is no body bytes available, the body's source will wake the driver.
    /// - If the cause is an exhausted window, the peer's next `WINDOW_UPDATE` (arriving on the read
    ///   path) will wake the driver and the next tick will retry.
    fn poll_emit_one_data(
        &mut self,
        stream_id: u32,
        send: &mut SendCursor,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        // Cloned once so `transition_to_trailers` can drain `pending_trailers` without
        // contending with the body poll's `&mut self.body_scratch` borrow.
        let shared = Arc::clone(
            &self
                .streams
                .get(&stream_id)
                .expect("stream id present; cursor was taken from this entry")
                .shared,
        );

        // Fast path — body already drained (poll_read returned Ready(0) on a prior tick)
        // OR we've already emitted the declared body length. Transition without polling.
        // The Content-Length-known check is what lets us close out a stream whose window
        // just barely sufficed for the body without waiting on a superfluous WINDOW_UPDATE
        // to detect EOF.
        if send.body.is_none() || send.body_len == Some(send.body_bytes_emitted) {
            end_of_body(send, &shared, stream_id);
            return Poll::Ready(Ok(()));
        }

        // Budget = min(body_scratch capacity, stream send window, connection send window).
        let stream_window = self.streams.get(&stream_id).map_or(0, |e| e.send_window);
        let budget = stream_window.min(self.connection_send_window);
        if budget <= 0 {
            // Windows exhausted; peer WINDOW_UPDATE via the read path will wake us.
            return Poll::Pending;
        }
        let cap = usize::try_from(budget)
            .unwrap_or(usize::MAX)
            .min(self.body_scratch.len());

        let body = send.body.as_mut().expect("checked above");
        let n = match Pin::new(body).poll_read(cx, &mut self.body_scratch[..cap]) {
            Poll::Ready(result) => result?,
            Poll::Pending => {
                // No bytes available now; the body registered its own waker (upgrade
                // reader awaiting a handler write, or an async body). Record the park so
                // `has_pending_outbound_progress` lets the driver sleep instead of
                // busy-looping — the external wake is what resumes us.
                send.phase = SendPhase::Body { parked: true };
                return Poll::Pending;
            }
        };
        send.phase = SendPhase::Body { parked: false };
        if n == 0 {
            // Body drained via a 0-byte read (unknown-length body reached EOF).
            end_of_body(send, &shared, stream_id);
            return Poll::Ready(Ok(()));
        }

        let n_u32 = u32::try_from(n).expect("read n <= body_scratch.len() fits u32");
        log::trace!("h2 emit: DATA stream={stream_id} len={n} end_stream=false");
        self.queue_frame(frame::data::encoded_prefix_len(0), |buf| {
            // Never END_STREAM here; trailers / empty-DATA carries END_STREAM.
            frame::data::encode_prefix(stream_id, false, n_u32, 0, buf)
        });
        self.write_buf.extend_from_slice(&self.body_scratch[..n]);

        // Charge both windows. `n <= body_scratch.len() = MAX_DATA_CHUNK_SIZE` which
        // comfortably fits i64 without wraparound.
        let charge = i64::try_from(n).expect("n <= body_scratch.len() fits i64");
        self.connection_send_window -= charge;
        if let Some(entry) = self.streams.get_mut(&stream_id) {
            entry.send_window -= charge;
        }
        send.body_bytes_emitted += n as u64;

        // If body length is known and we just emitted the last byte, transition within
        // this call so `advance_one_send`'s phase loop can fall through to
        // `emit_trailers_or_end_stream` — otherwise we'd park in Body and wait for an
        // external wake to notice EOF, which never comes (peer has nothing more to send).
        if send.body_len == Some(send.body_bytes_emitted) {
            end_of_body(send, &shared, stream_id);
        }

        Poll::Ready(Ok(()))
    }

    /// Emit either a trailing HEADERS (with `END_STREAM`) if the response has trailers, or
    /// an empty `DATA(END_STREAM)` frame as the stream terminator. Transitions to
    /// `Complete` so the next tick fires the conn-task completion signal.
    fn emit_trailers_or_end_stream(&mut self, stream_id: u32, send: &mut SendCursor) {
        if let Some(trailers) = send.trailers.take() {
            // Encode trailers via the per-connection HPACK encoder. Trailers carry no
            // pseudo-headers (response status/etc. are already in the HEADERS frame).
            let mut block = Vec::new();
            self.hpack_encoder.encode(
                &FieldSection::new(PseudoHeaders::default(), &trailers),
                &mut block,
            );
            // Trailing HEADERS: END_HEADERS=true, END_STREAM=true.
            let block_len_u32 = u32::try_from(block.len()).expect("trailers block fits u32");
            log::trace!(
                "h2 emit: HEADERS (trailers) stream={stream_id} len={} end_headers=true \
                 end_stream=true",
                block.len(),
            );
            self.queue_frame(frame::headers::encoded_prefix_len(0, false), |buf| {
                frame::headers::encode_prefix(stream_id, true, true, None, block_len_u32, 0, buf)
            });
            self.write_buf.extend_from_slice(&block);
        } else {
            // No trailers — empty DATA frame with END_STREAM as the stream terminator.
            log::trace!("h2 emit: DATA stream={stream_id} len=0 end_stream=true (terminator)");
            self.queue_frame(frame::data::encoded_prefix_len(0), |buf| {
                frame::data::encode_prefix(stream_id, true, 0, 0, buf)
            });
        }
        send.phase = SendPhase::Complete;
    }
}

/// The cursor's current body has drained. For an upgrade whose prelude just finished,
/// this is the handoff point: resolve the `SubmitSend` future (the prelude is fully framed,
/// so the conn task returns and the upgrade handler runs) and swap in the per-stream
/// outbound queue as the continuation source, staying in `Body` phase. Resolving the submit
/// future here is *not* a send-half close — the lifecycle stays `UpgradeOpen` and the stream
/// streams on. Otherwise — a non-upgrade body, or an upgrade whose continuation has now also
/// drained (handler closed) — fall through to `transition_to_trailers` and terminate the
/// stream.
fn end_of_body(send: &mut SendCursor, shared: &Arc<StreamState>, stream_id: u32) {
    if send.is_upgrade && !send.continuation_started {
        if !send.completion_signaled {
            log::trace!(
                "h2 stream {stream_id}: upgrade — prelude framed, resolving SubmitSend and \
                 switching to outbound continuation"
            );
            signal_submit_resolved(shared, Ok(()));
            send.completion_signaled = true;
        }
        send.body = Some(
            Body::new_streaming(H2OutboundReader::new(shared.clone(), stream_id), None).into_h2(),
        );
        send.body_len = None;
        send.body_bytes_emitted = 0;
        send.continuation_started = true;
        send.phase = SendPhase::Body { parked: false };
    } else {
        transition_to_trailers(send, shared);
    }
}

/// Body drained — pull trailers off the body or off the `UpgradeClosing` lifecycle
/// variant's `pending_trailers` slot (the out-of-band mailbox
/// [`H2Connection::submit_trailers`][crate::h2::H2Connection::submit_trailers] writes
/// into), drop the body handle, transition to `Trailers`.
fn transition_to_trailers(send: &mut SendCursor, shared: &StreamState) {
    if send.trailers.is_none() {
        send.trailers = send.body.as_mut().and_then(H2Body::trailers);
    }
    if send.trailers.is_none() {
        // Drain pending_trailers if the lifecycle is `UpgradeClosing`. On any other
        // variant there are no out-of-band trailers — `submit_trailers` only stages onto
        // `UpgradeClosing`.
        let mut lifecycle = shared.lifecycle_lock();
        if let crate::h2::lifecycle::StreamLifecycle::UpgradeClosing {
            pending_trailers, ..
        } = &mut *lifecycle
        {
            send.trailers = pending_trailers.take();
        }
    }
    send.body = None;
    send.phase = SendPhase::Trailers;
}

/// Resolve the conn task's [`SubmitSend`][super::super::SubmitSend] future: store the
/// result, flip `submit_resolved`, wake its waker. Does **not** mark the send half closed
/// — that's [`StreamLifecycle::mark_send_closed`], a separate lifecycle transition.
/// Free fn so the driver can call it from inside an `&mut self` borrow chain without a
/// re-lookup.
fn signal_submit_resolved(state: &StreamState, result: io::Result<()>) {
    *state
        .send
        .completion_result
        .lock()
        .expect("completion_result mutex poisoned") = Some(result);
    state.send.submit_resolved.store(true, Ordering::Release);
    state.send.completion_waker.wake();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Headers;

    /// Build a minimal `SendCursor` in `Body` phase with no body and no trailers — the
    /// natural state for an empty-body response that's about to transition.
    fn cursor_in_body() -> SendCursor {
        SendCursor {
            encoded_headers: Vec::new(),
            headers_offset: 0,
            has_body: false,
            body: None,
            body_len: None,
            body_bytes_emitted: 0,
            trailers: None,
            phase: SendPhase::Body { parked: false },
            is_upgrade: false,
            continuation_started: false,
            completion_signaled: false,
        }
    }

    fn one_trailer(name: &'static str, value: &'static str) -> Headers {
        let mut h = Headers::new();
        h.insert(name, value);
        h
    }

    use crate::h2::lifecycle::StreamLifecycle;

    /// Regression for the trailers-stranding bug fixed by the FSM refactor: when neither
    /// the cursor nor the body produces trailers, `transition_to_trailers` must fall back
    /// to draining the `UpgradeClosing` variant's `pending_trailers` payload (the
    /// out-of-band mailbox
    /// [`H2Connection::submit_trailers`][crate::h2::H2Connection::submit_trailers] writes
    /// into). Without this fallback, trailers submitted between driver ticks while a
    /// cursor was parked in `Body` were lost.
    #[test]
    fn pending_trailers_picked_up_when_cursor_and_body_have_none() {
        let shared = StreamState::default();
        *shared.lifecycle_lock() = StreamLifecycle::UpgradeClosing {
            recv_eof: true,
            pending_trailers: Some(one_trailer("grpc-status", "0")),
        };

        let mut send = cursor_in_body();
        transition_to_trailers(&mut send, &shared);

        let trailers = send
            .trailers
            .expect("trailers drained from UpgradeClosing payload");
        assert_eq!(trailers.get_str("grpc-status"), Some("0"));
        assert_eq!(send.phase, SendPhase::Trailers);
        assert!(send.body.is_none());
        let drained = matches!(
            &*shared.lifecycle_lock(),
            StreamLifecycle::UpgradeClosing {
                pending_trailers: None,
                ..
            },
        );
        assert!(drained, "pending_trailers must be drained, not just copied");
    }

    /// Pre-staged cursor trailers win over the `UpgradeClosing` payload fallback. The
    /// payload isn't drained unnecessarily — leaving it populated avoids leaking
    /// trailers across stream boundaries.
    #[test]
    fn cursor_trailers_preserved_pending_not_drained() {
        let shared = StreamState::default();
        *shared.lifecycle_lock() = StreamLifecycle::UpgradeClosing {
            recv_eof: true,
            pending_trailers: Some(one_trailer("from-pending", "ignored")),
        };

        let mut send = cursor_in_body();
        send.trailers = Some(one_trailer("from-cursor", "kept"));
        transition_to_trailers(&mut send, &shared);

        let trailers = send.trailers.expect("cursor trailers preserved");
        assert_eq!(trailers.get_str("from-cursor"), Some("kept"));
        assert_eq!(trailers.get_str("from-pending"), None);
        assert_eq!(send.phase, SendPhase::Trailers);
        let still_populated = matches!(
            &*shared.lifecycle_lock(),
            StreamLifecycle::UpgradeClosing {
                pending_trailers: Some(_),
                ..
            },
        );
        assert!(
            still_populated,
            "UpgradeClosing payload must not be drained when cursor already had trailers",
        );
    }

    /// Empty trailers from every source: cursor's `trailers` stays `None` so the send
    /// pump emits an empty `DATA(END_STREAM)` as the terminator instead of trailing
    /// HEADERS. Uses a non-upgrade lifecycle (`Sending`) so there's no payload to drain
    /// in the first place.
    #[test]
    fn no_trailers_anywhere_leaves_cursor_trailers_none() {
        let shared = StreamState::default();
        *shared.lifecycle_lock() = StreamLifecycle::Sending { recv_eof: true };
        let mut send = cursor_in_body();
        transition_to_trailers(&mut send, &shared);

        assert!(send.trailers.is_none(), "no trailers from any source");
        assert_eq!(send.phase, SendPhase::Trailers);
        assert!(send.body.is_none());
    }
}
