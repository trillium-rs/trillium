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
        transport::{StreamState, Submission},
    },
    headers::hpack::{FieldSection, HpackEncoder, PseudoHeaders},
};
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    io,
    pin::Pin,
    sync::atomic::Ordering,
    task::{Context, Poll, ready},
};

/// Driver-private state for an in-progress response on a single stream. Never observed
/// concurrently — only the driver task touches this.
#[derive(Debug)]
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
    /// `true` if this stream is in extended-CONNECT upgrade mode (RFC 8441):
    /// signal [`SubmitSend`][super::super::SubmitSend] completion the moment `END_HEADERS`
    /// goes out instead of waiting for `END_STREAM`, so the runtime can dispatch
    /// [`Handler::upgrade`][trillium::Handler::upgrade] while the streaming body keeps
    /// pumping bytes from [`SendState::outbound`][super::super::transport::SendState] into
    /// DATA frames in the background.
    is_upgrade: bool,
    /// `true` once `signal_send_completion` has been called for this cursor — prevents the
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
        let has_body = submission.body.is_some();
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
            completion_signaled: false,
        }
    }
}

/// Position of a `SendCursor` in the response lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendPhase {
    /// Still emitting HEADERS + CONTINUATION fragments.
    Headers,
    /// HEADERS done; pumping body bytes into DATA frames.
    Body,
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
        if self.state != DriverState::Running {
            return;
        }
        let stream_ids: Vec<u32> = self.streams.keys().copied().collect();
        for stream_id in stream_ids {
            self.advance_one_send(stream_id, cx);
        }
    }

    /// True if any active stream has more outbound work that could make progress on the next
    /// tick — a `SendCursor` mid-Headers / Trailers / Complete (no flow control gates these),
    /// or a `SendCursor` in Body with a positive per-stream send window AND a positive
    /// connection send window. Used by [`park`][super::H2Driver::park] to keep the driver
    /// awake when there are body bytes left to emit and budget to emit them with — the body
    /// source wouldn't wake us in that case (it already returned `Ready` on the prior poll),
    /// and the only frame the peer is obliged to send is a `WINDOW_UPDATE` once our budget
    /// runs out, not before.
    pub(super) fn has_pending_outbound_progress(&self) -> bool {
        if self.connection_send_window <= 0 {
            return false;
        }
        self.streams.values().any(|entry| match &entry.send {
            None => false,
            Some(send) => match send.phase {
                SendPhase::Headers | SendPhase::Trailers | SendPhase::Complete => true,
                SendPhase::Body => entry.send_window > 0,
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
                SendPhase::Body => match self.poll_emit_one_data(stream_id, &mut send, cx) {
                    Poll::Ready(Ok(())) => {
                        // Body returned Ready(N>0) (emitted DATA, still Body) or Ready(0)
                        // (transitioned to Trailers). On transition, run the new phase
                        // this tick; on stay-in-Body, yield to the next stream.
                        if send.phase == SendPhase::Body {
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
                    "h2 stream {stream_id}: skipping signal_send_completion (already signaled by \
                     upgrade path)"
                );
            } else {
                signal_send_completion(&entry.shared, result);
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

    /// Send pump's success-path completion. Signals the conn task's pending
    /// [`SubmitSend`][super::super::SubmitSend], then for **server** role removes the
    /// stream immediately (response done = stream done) and for **client** role defers
    /// removal until the recv side has also observed `END_STREAM` — the request being
    /// fully sent doesn't end the stream from the client's perspective; we still need to
    /// receive the response.
    ///
    /// The deferred client-role completion is finalized either by the peer's response
    /// reaching `END_STREAM` (via [`route_data`][super::recv] or the HEADERS-with-END_STREAM
    /// case in [`finalize_response_headers`][super::recv]) or by an error/RST path on
    /// either side (which routes through [`Self::complete_and_remove_stream`] directly).
    pub(super) fn finalize_send(&mut self, stream_id: u32) {
        if let Some(entry) = self.streams.get_mut(&stream_id) {
            let already_signaled = entry.send.as_ref().is_some_and(|c| c.completion_signaled);
            if !already_signaled {
                signal_send_completion(&entry.shared, Ok(()));
                if let Some(c) = entry.send.as_mut() {
                    c.completion_signaled = true;
                }
            }
        }
        match self.role {
            Role::Server => self.complete_and_remove_stream(stream_id, Ok(())),
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
        let send_done = entry.shared.send.completed.load(Ordering::Acquire);
        let recv_done = entry.shared.recv.eof.load(Ordering::Acquire);
        if send_done && recv_done {
            self.signal_close(stream_id, Ok(()));
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
            // Extended-CONNECT (RFC 8441): signal `SubmitSend` completion as
            // soon as the response HEADERS frame is on the wire so `Conn::send_h2`
            // returns and the runtime can dispatch `handler.upgrade(...)`. The body
            // (an `H2OutboundReader` over `SendState.outbound`) keeps streaming in the
            // background — the eventual `complete_and_remove_stream` call sees
            // `completion_signaled` and skips re-signaling.
            if send.is_upgrade && !send.completion_signaled {
                if let Some(entry) = self.streams.get(&stream_id) {
                    log::trace!(
                        "h2 stream {stream_id}: upgrade — signaling SubmitSend completion at \
                         END_HEADERS"
                    );
                    signal_send_completion(&entry.shared, Ok(()));
                }
                send.completion_signaled = true;
            }

            send.phase = if send.has_body {
                SendPhase::Body
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
        // Fast path — body already drained (poll_read returned Ready(0) on a prior tick)
        // OR we've already emitted the declared body length. Transition without polling.
        // The Content-Length-known check is what lets us close out a stream whose window
        // just barely sufficed for the body without waiting on a superfluous WINDOW_UPDATE
        // to detect EOF.
        if send.body.is_none() || send.body_len == Some(send.body_bytes_emitted) {
            transition_to_trailers(send);
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
        let n = ready!(Pin::new(body).poll_read(cx, &mut self.body_scratch[..cap]))?;
        if n == 0 {
            // Body drained via a 0-byte read (unknown-length body reached EOF).
            transition_to_trailers(send);
            return Poll::Ready(Ok(()));
        }

        let n_u32 = u32::try_from(n).expect("read n <= body_scratch.len() fits u32");
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
            transition_to_trailers(send);
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
            self.queue_frame(frame::headers::encoded_prefix_len(0, false), |buf| {
                frame::headers::encode_prefix(stream_id, true, true, None, block_len_u32, 0, buf)
            });
            self.write_buf.extend_from_slice(&block);
        } else {
            // No trailers — empty DATA frame with END_STREAM as the stream terminator.
            self.queue_frame(frame::data::encoded_prefix_len(0), |buf| {
                frame::data::encode_prefix(stream_id, true, 0, 0, buf)
            });
        }
        send.phase = SendPhase::Complete;
    }
}

/// Body drained — pull trailers off the body, drop the body handle, transition to
/// `Trailers`.
///
/// The `is_none()` guard preserves trailers set out-of-band by
/// [`H2Connection::submit_trailers`][crate::h2::H2Connection::submit_trailers], whose
/// body reports no trailers of its own.
fn transition_to_trailers(send: &mut SendCursor) {
    if send.trailers.is_none() {
        send.trailers = send.body.as_mut().and_then(H2Body::trailers);
    }
    send.body = None;
    send.phase = SendPhase::Trailers;
}

/// Store the send result on `StreamState`, flip `completed`, wake the conn-task waker.
/// Free fn so the driver can call it from inside an `&mut self` borrow chain without a
/// re-lookup.
fn signal_send_completion(state: &StreamState, result: io::Result<()>) {
    *state
        .send
        .completion_result
        .lock()
        .expect("completion_result mutex poisoned") = Some(result);
    state.send.completed.store(true, Ordering::Release);
    state.send.completion_waker.wake();
}
