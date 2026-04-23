//! Send pump: turns conn-task-submitted responses ([`SendCursor`]s) into HEADERS / DATA /
//! trailing-HEADERS frame bytes in `H2Acceptor.write_buf`, and signals completion back to the
//! conn task once the response is fully on the wire.
//!
//! Picks up new submissions from per-stream `StreamState.send.submission` slots in the
//! parent's `service_handler_signals`. Per-tick, advances each active send by one frame
//! (with the §6.10 exception: HEADERS+CONTINUATION runs to `END_HEADERS` without yielding to
//! other streams).
//!
//! All methods are on [`super::H2Acceptor`].

use super::H2Acceptor;
use crate::{
    Body, Headers,
    h2::{
        frame,
        transport::{StreamState, Submission},
    },
    headers::hpack::{self, FieldSection, PseudoHeaders},
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
    /// Pre-encoded HEADERS bytes (HPACK output from the conn task), chunked into HEADERS +
    /// CONTINUATION frames as `peer_max_frame_size` allows.
    encoded_headers: Vec<u8>,
    /// Offset into `encoded_headers` of the first byte not yet emitted.
    headers_offset: usize,
    /// Whether this stream's response carries a body. Decides whether the final HEADERS
    /// fragment carries `END_STREAM` (no body, no trailers) or whether we transition to
    /// the Body phase next.
    has_body: bool,
    /// Body source. Driver polls `body.poll_read` to fill DATA frames; transitions to None
    /// once drained (a 0-byte read).
    body: Option<Body>,
    /// Trailers, populated from `body.trailers()` once the body is fully drained.
    trailers: Option<Headers>,
    /// Where we are in the response.
    phase: SendPhase,
}

impl SendCursor {
    pub(super) fn new(submission: Submission) -> Self {
        let has_body = submission.body.is_some();
        Self {
            encoded_headers: submission.encoded_headers,
            headers_offset: 0,
            has_body,
            body: submission.body,
            trailers: None,
            phase: SendPhase::Headers,
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

impl<T> H2Acceptor<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    /// Advance every active send by at most one step per tick (headers fragments are
    /// emitted atomically per stream — RFC 9113 §6.10 forbids interleaving
    /// HEADERS+CONTINUATION with any other frame on any other stream). Body reads that
    /// return Pending leave the cursor in place; the body's source will wake the driver
    /// task when bytes are available.
    pub(super) fn advance_outbound_sends(&mut self, cx: &mut Context<'_>) {
        let stream_ids: Vec<u32> = self.streams.keys().copied().collect();
        for stream_id in stream_ids {
            self.advance_one_send(stream_id, cx);
        }
    }

    /// Advance one stream's `SendCursor` by one frame's worth of work, with the §6.10
    /// exception: in `Headers` phase we keep emitting fragments back-to-back until
    /// `END_HEADERS` is set. Other phases emit at most one frame per tick to keep streams
    /// roughly fair.
    fn advance_one_send(&mut self, stream_id: u32, cx: &mut Context<'_>) {
        let Some(mut send) = self
            .streams
            .get_mut(&stream_id)
            .and_then(|e| e.send.take())
        else {
            return;
        };

        loop {
            match send.phase {
                SendPhase::Headers => {
                    // §6.10 forbids interleaving HEADERS+CONTINUATION with any other frame,
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
                    self.complete_and_remove_stream(stream_id, Ok(()));
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
    fn complete_and_remove_stream(&mut self, stream_id: u32, result: io::Result<()>) {
        if let Some(entry) = self.streams.remove(&stream_id) {
            signal_send_completion(&entry.shared, result);
        }
        self.connection.streams_lock().remove(&stream_id);
    }

    /// Emit one HEADERS or CONTINUATION fragment from `send.encoded_headers`. Transitions
    /// `send.phase` to `Body` / `Trailers` / `Complete` once `END_HEADERS` is set. The
    /// first fragment is HEADERS; subsequent fragments (when `headers_offset > 0`) are
    /// CONTINUATION.
    fn emit_one_headers_fragment(&mut self, stream_id: u32, send: &mut SendCursor) {
        let max_payload = self.peer_max_frame_size as usize;
        let remaining = send.encoded_headers.len() - send.headers_offset;
        let chunk_len = remaining.min(max_payload);
        let end_headers = chunk_len == remaining;
        let is_first = send.headers_offset == 0;
        let chunk_len_u32 = u32::try_from(chunk_len).expect("chunk_len <= peer_max_frame_size u32");

        if is_first {
            // Final HEADERS fragment with no body and no trailers carries END_STREAM.
            let end_stream = end_headers && !send.has_body;
            let prefix_len = frame::headers::encoded_prefix_len(0, false);
            let start = self.write_buf.len();
            self.write_buf.resize(start + prefix_len, 0);
            frame::headers::encode_prefix(
                stream_id,
                end_stream,
                end_headers,
                None,
                chunk_len_u32,
                0,
                &mut self.write_buf[start..],
            )
            .expect("buffer sized from encoded_prefix_len");
        } else {
            let prefix_len = frame::continuation::ENCODED_PREFIX_LEN;
            let start = self.write_buf.len();
            self.write_buf.resize(start + prefix_len, 0);
            frame::continuation::encode_prefix(
                stream_id,
                end_headers,
                chunk_len_u32,
                &mut self.write_buf[start..],
            )
            .expect("buffer sized from ENCODED_PREFIX_LEN");
        }

        // Append the header-block fragment payload.
        self.write_buf.extend_from_slice(
            &send.encoded_headers[send.headers_offset..send.headers_offset + chunk_len],
        );
        send.headers_offset += chunk_len;
        self.write_flush_pending = true;

        if end_headers {
            send.phase = if send.has_body {
                SendPhase::Body
            } else {
                // The single HEADERS fragment carried END_STREAM (or final CONTINUATION
                // did not — but our encoder above only sets END_STREAM on the *first*
                // fragment, so for the multi-fragment + no-body case we'd need an extra
                // empty DATA. That case is unreachable today: response headers always fit
                // comfortably in one peer-default 16 KiB frame, but still — guard with a
                // Trailers transition that the next tick will turn into an empty
                // DATA(END_STREAM).
                if is_first {
                    SendPhase::Complete
                } else {
                    SendPhase::Trailers
                }
            };
        }
    }

    /// Poll the body for one DATA chunk. On `Ready(Ok(0))`, takes trailers off the body and
    /// transitions to `Trailers`. On `Ready(Ok(n))`, emits one DATA frame (no
    /// `END_STREAM`). On `Pending`, the cursor stays in `Body` — body's source will wake
    /// the driver task.
    fn poll_emit_one_data(
        &mut self,
        stream_id: u32,
        send: &mut SendCursor,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        let Some(body) = send.body.as_mut() else {
            // Body already drained but somehow we're still in Body phase — treat as 0-byte
            // EOF.
            send.phase = SendPhase::Trailers;
            return Poll::Ready(Ok(()));
        };

        let n = ready!(Pin::new(body).poll_read(cx, &mut self.body_scratch))?;
        if n == 0 {
            // Body drained. Take trailers off it, drop the body, transition.
            send.trailers = send.body.as_mut().and_then(Body::trailers);
            send.body = None;
            send.phase = SendPhase::Trailers;
            return Poll::Ready(Ok(()));
        }

        let n_u32 = u32::try_from(n).expect("read n <= peer_max_frame_size u32");
        let prefix_len = frame::data::encoded_prefix_len(0);
        let start = self.write_buf.len();
        self.write_buf.resize(start + prefix_len, 0);
        frame::data::encode_prefix(
            stream_id,
            false, // never END_STREAM here; trailers / empty-DATA carries END_STREAM
            n_u32,
            0,
            &mut self.write_buf[start..],
        )
        .expect("buffer sized from encoded_prefix_len");
        self.write_buf.extend_from_slice(&self.body_scratch[..n]);
        self.write_flush_pending = true;
        Poll::Ready(Ok(()))
    }

    /// Emit either a trailing HEADERS (with `END_STREAM`) if the response has trailers, or
    /// an empty `DATA(END_STREAM)` frame as the stream terminator. Transitions to
    /// `Complete` so the next tick fires the conn-task completion signal.
    fn emit_trailers_or_end_stream(&mut self, stream_id: u32, send: &mut SendCursor) {
        if let Some(trailers) = send.trailers.take() {
            // Encode trailers via the static-or-literal HPACK encoder. Trailers carry no
            // pseudo-headers (response status/etc. are already in the HEADERS frame).
            let mut block = Vec::new();
            hpack::encode(
                &FieldSection::new(PseudoHeaders::default(), &trailers),
                &mut block,
            );
            // Trailing HEADERS: END_HEADERS=true, END_STREAM=true.
            let block_len_u32 = u32::try_from(block.len()).expect("trailers block fits u32");
            let prefix_len = frame::headers::encoded_prefix_len(0, false);
            let start = self.write_buf.len();
            self.write_buf.resize(start + prefix_len, 0);
            frame::headers::encode_prefix(
                stream_id,
                true,
                true,
                None,
                block_len_u32,
                0,
                &mut self.write_buf[start..],
            )
            .expect("buffer sized from encoded_prefix_len");
            self.write_buf.extend_from_slice(&block);
        } else {
            // No trailers — empty DATA frame with END_STREAM as the stream terminator.
            let prefix_len = frame::data::encoded_prefix_len(0);
            let start = self.write_buf.len();
            self.write_buf.resize(start + prefix_len, 0);
            frame::data::encode_prefix(stream_id, true, 0, 0, &mut self.write_buf[start..])
                .expect("buffer sized from encoded_prefix_len");
        }
        self.write_flush_pending = true;
        send.phase = SendPhase::Complete;
    }
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
