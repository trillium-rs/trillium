use super::super::{CloseOutcome, ClosedReason, H2Driver, MAX_FLOW_CONTROL_WINDOW, frame_slice};
use crate::h2::{H2ErrorCode, frame::FRAME_HEADER_LEN, role::Role, stream_state::StreamEvent};
use futures_lite::io::{AsyncRead, AsyncWrite};

impl<T: AsyncRead + AsyncWrite + Unpin + Send> H2Driver<T> {
    pub(super) fn handle_rst_stream(
        &mut self,
        stream_id: u32,
        error_code: H2ErrorCode,
    ) -> Result<(), CloseOutcome> {
        // `RST_STREAM` on an idle stream is a connection-level `PROTOCOL_ERROR`;
        // on a closed or active stream it's benign.
        if stream_id > self.last_peer_stream_id && !self.streams.contains_key(&stream_id) {
            return Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError));
        }
        if let Some(entry) = self.streams.get(&stream_id) {
            // Move the stream to the terminal `Closed{Reset}` state before tearing down, so
            // a handler parked on it re-polls to EOF / `BrokenPipe` rather than accepting
            // writes into a ring the driver has stopped draining (silent data loss on an
            // upgraded stream). `signal_close` then fires the conn-task wakers so the
            // parked handler actually observes the close.
            let recv_closed = entry.shared.lifecycle_lock().recv_closed();
            let _ = entry.shared.apply_event(StreamEvent::RecvReset(error_code));
            if self.role == Role::Client && recv_closed {
                // The full response — including any trailers the driver already stashed —
                // arrived before this RST, which only aborts our still-open send half (a
                // server that responds without consuming the request body resets it with
                // NoError). Keep the stream in the map exactly as a clean close does, so
                // the application can still read the body to EOF and collect its trailers;
                // it's GC'd on transport drop.
                self.signal_close(stream_id, Err(std::io::Error::other("peer RST_STREAM")));
            } else {
                self.complete_and_remove_stream(
                    stream_id,
                    Err(std::io::Error::other("peer RST_STREAM")),
                );
            }
        } else {
            // Already closed from our side; still record (idempotent) so later
            // stray peer frames on this id map to the right error category.
            self.closed_streams.record(stream_id, ClosedReason::Reset);
        }
        Ok(())
    }

    /// A DATA frame arrived — copy its payload into the matching stream's recv buffer and
    /// wake the handler. Padding bytes are part of the already-read frame body and are
    /// skipped (they're in the buffer but not pushed).
    ///
    /// Stream-state errors:
    /// - **Idle** (`stream_id` > `last_peer_stream_id`): DATA on an unopened stream is a
    ///   connection-level `PROTOCOL_ERROR`.
    /// - **Closed** (`stream_id` ≤ `last_peer_stream_id`, not in active map): stream-level
    ///   `RST_STREAM(STREAM_CLOSED)`. Sent after-the-fact — peer has already written this frame and
    ///   we've already read it off the wire.
    /// - **Half-closed remote** (in map, lifecycle already recv-closed): same stream-level
    ///   `STREAM_CLOSED`.
    ///
    /// Flow-control accounting: the entire DATA payload (including pad-length byte +
    /// padding) counts against both the per-stream and connection-level recv windows
    /// ([`Inflow::take`][super::super::inflow::Inflow::take]). Both are enforced strictly — a peer
    /// that sends past the window we granted earns a connection-level `FLOW_CONTROL_ERROR`.
    /// The `avail + buffered ≤ target` invariant means the granted window doubles as the
    /// per-stream buffer bound, so there's no separate cap to reconcile.
    pub(super) fn route_data(
        &mut self,
        stream_id: u32,
        end_stream: bool,
        data_length: u32,
        payload_start: usize,
        total: usize,
    ) -> Result<(), CloseOutcome> {
        // Flow-controlled byte count is the entire frame payload — data + pad-length byte
        // (if present) + padding. The frame header is not flow-controlled. Padding bytes
        // past `data_length` stay in `read_buf` but aren't copied into the recv ring.
        let flow_controlled = i64::try_from(total - FRAME_HEADER_LEN)
            .map_err(|_| CloseOutcome::Protocol(H2ErrorCode::FrameSizeError))?;

        // Connection-level accounting runs regardless of stream state. A protocol-respecting
        // peer can't exceed what we granted (SETTINGS doesn't alter the connection window), so a
        // rejected take is unambiguous peer overrun — connection-level FLOW_CONTROL_ERROR.
        if !self.connection_inflow.take(flow_controlled) {
            return Err(CloseOutcome::Protocol(H2ErrorCode::FlowControlError));
        }

        if !self.streams.contains_key(&stream_id) {
            return if stream_id > self.last_peer_stream_id {
                // Idle — never opened; connection error.
                Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError))
            } else {
                // Closed — stream-level.
                self.queue_rst_stream(stream_id, H2ErrorCode::StreamClosed);
                Ok(())
            };
        }

        let entry = self
            .streams
            .get_mut(&stream_id)
            .expect("checked above under shared borrow");
        entry.received_body_len += u64::from(data_length);
        // RFC 9113 §8.1.2.6: a declared content-length must equal the total DATA payload. A
        // running total past the declared length is a violation as soon as it's seen; a
        // total that still disagrees at END_STREAM is one too. Either way, stream-level
        // PROTOCOL_ERROR.
        let content_length_mismatch = entry.expected_content_length.is_some_and(|expected| {
            entry.received_body_len > expected
                || (end_stream && entry.received_body_len != expected)
        });
        let state = entry.shared.clone();

        // Strict per-stream flow control, enforced only while the stream is still open for
        // receiving and not already failing the content-length check — a closing stream's window
        // is moot and it's about to be reset regardless. A compliant peer never overruns the
        // window we granted, so a rejected take is unambiguous overrun.
        if !content_length_mismatch
            && !state.lifecycle_lock().recv_closed()
            && !entry.stream_inflow.take(flow_controlled)
        {
            return Err(CloseOutcome::Protocol(H2ErrorCode::FlowControlError));
        }

        // Half-closed remote / closed: peer already sent END_STREAM on this stream; any DATA after
        // that is stream-level STREAM_CLOSED. Flow-control accounting above still applies.
        if state.lifecycle_lock().recv_closed() {
            self.queue_rst_stream(stream_id, H2ErrorCode::StreamClosed);
            self.complete_and_remove_stream(
                stream_id,
                Err(std::io::Error::other("DATA after END_STREAM on h2 stream")),
            );
            return Ok(());
        }

        if content_length_mismatch {
            self.queue_rst_stream(stream_id, H2ErrorCode::ProtocolError);
            self.complete_and_remove_stream(
                stream_id,
                Err(std::io::Error::other("h2 content-length mismatch")),
            );
            return Ok(());
        }

        let data = frame_slice(
            &self.read_buf[self.read_start..],
            payload_start,
            data_length,
            total,
        )?;

        {
            let mut recv = state.recv.buf.lock().expect("recv buf mutex poisoned");
            // Strict stream flow control bounds the buffer to the window target (the
            // `avail + buffered ≤ target` invariant), so a compliant peer can't exceed it —
            // asserted in debug to catch bookkeeping drift. `max_stream_recv_window_size` is
            // coerced ≥ the initial at config snapshot, so it's the true upper bound on the target.
            debug_assert!(
                recv.len() + data.len() <= self.config.max_stream_recv_window_size() as usize,
                "recv buffer exceeded the stream window target under strict flow control"
            );
            if !data.is_empty() {
                recv.extend_live(data);
            }
        }
        // Transition recv-closed *after* the data is in the ring, so a reader that observes
        // recv-closed is guaranteed to see the buffered bytes first.
        if end_stream {
            let _ = state.apply_event(StreamEvent::RecvData { end_stream: true });
        }
        state.recv.waker.wake();
        // Peer END_STREAM may be the second half of "both halves done". If our response already
        // completed (lifecycle `HalfClosedLocal`), this transition reached `Closed` and we tear
        // down now; otherwise `finalize_send` closes it when the response completes.
        if end_stream {
            self.close_if_both_done(stream_id);
        }
        Ok(())
    }

    /// Apply a peer `WINDOW_UPDATE`. Connection-level updates (`stream_id == 0`) credit
    /// the driver's `connection_send_window`; stream-level updates credit the matching
    /// `StreamEntry.send_window`.
    ///
    /// Every flow-control window is bounded at `2^31 - 1`. An increment that would push a
    /// window past that maximum is a `FLOW_CONTROL_ERROR`, handled at the appropriate
    /// level:
    /// - Connection window overflow → connection-level GOAWAY (via the returned error).
    /// - Stream window overflow → stream-level `RST_STREAM`, stream cleanup, connection continues.
    ///
    /// A `WINDOW_UPDATE` on a stream we don't know is benign (the peer may send one after
    /// the stream has closed): log and move on.
    pub(super) fn apply_window_update(
        &mut self,
        stream_id: u32,
        increment: u32,
    ) -> Result<(), CloseOutcome> {
        let inc = i64::from(increment);

        if stream_id == 0 {
            let new = self.connection_send_window + inc;
            if new > MAX_FLOW_CONTROL_WINDOW {
                return Err(CloseOutcome::Protocol(H2ErrorCode::FlowControlError));
            }
            self.connection_send_window = new;
            return Ok(());
        }

        let Some(entry) = self.streams.get_mut(&stream_id) else {
            // WINDOW_UPDATE on an idle stream is a connection error. On a closed
            // stream it's benign (the peer may credit a just-closed stream before it
            // observed our END_STREAM).
            if stream_id > self.last_peer_stream_id {
                return Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError));
            }
            log::trace!("WINDOW_UPDATE on closed stream {stream_id} — ignoring");
            return Ok(());
        };
        let new = entry.send_window + inc;
        if new > MAX_FLOW_CONTROL_WINDOW {
            // Stream-level overflow. RST + cleanup + signal any pending send.
            self.queue_rst_stream(stream_id, H2ErrorCode::FlowControlError);
            self.complete_and_remove_stream(
                stream_id,
                Err(std::io::Error::other(
                    "peer WINDOW_UPDATE overflowed stream send window",
                )),
            );
            return Ok(());
        }
        entry.send_window = new;
        Ok(())
    }
}
