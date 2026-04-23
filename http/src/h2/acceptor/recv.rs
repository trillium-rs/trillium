//! Receive side of the HTTP/2 driver: frame reading, dispatch, malformed-request
//! `RST_STREAM`, DATA routing into per-stream recv rings, peer SETTINGS / `WINDOW_UPDATE`
//! application.
//!
//! HEADERS + CONTINUATION accumulation and HPACK-decoded HEADERS finalization (request /
//! response / trailers) live in [`headers`].
//!
//! All methods are on [`super::H2Driver`] — split off here to keep the driver's send and
//! receive logic in separate files. Visibility-wise, this child module reaches up via
//! `super::*` for everything it needs from the parent.

mod headers;

use super::{
    Action, CloseOutcome, ClosedReason, H2Driver, MAX_BUFFER_SIZE, MAX_FLOW_CONTROL_WINDOW,
    ReadPhase, Role, frame_slice,
};
use crate::h2::{
    H2ErrorCode, H2Settings,
    frame::{FRAME_HEADER_LEN, Frame, FrameDecodeError, FrameHeader},
};
use futures_lite::io::{AsyncRead, AsyncWrite};
pub(super) use headers::PendingHeaders;
use std::{
    sync::atomic::Ordering,
    task::{Context, Poll, ready},
};

/// The client connection preface (RFC 9113 §3.4). 24 bytes the client MUST send before any
/// HTTP/2 frames.
pub(super) const CLIENT_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

impl<T> H2Driver<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    /// Advance the read side by one frame. Accumulates bytes, and once a complete frame is
    /// available, dispatches it and returns the resulting action.
    ///
    /// Always returns after handling one frame (even on `Action::Continue`) so the outer
    /// loop gets a chance to flush any outbound bytes that dispatch queued — holding them
    /// in `write_buf` across reads would deadlock against a peer that's waiting for an ACK
    /// before sending its next frame.
    pub(super) fn poll_advance_read(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Action, CloseOutcome>> {
        // Make sure we've at least decoded the header and know how much payload to expect.
        let total = match self.read_phase {
            ReadPhase::NeedHeader => {
                ready!(self.poll_fill_to(FRAME_HEADER_LEN, cx)).map_err(CloseOutcome::Io)?;
                let header = FrameHeader::decode(&self.read_buf[..FRAME_HEADER_LEN])
                    .expect("FRAME_HEADER_LEN bytes already filled");
                // RFC 9113 §4.2: a frame whose length exceeds the receiver-advertised
                // `SETTINGS_MAX_FRAME_SIZE` is a `FRAME_SIZE_ERROR`. We also enforce
                // [`MAX_BUFFER_SIZE`] as a DoS guard — it's the higher of the two limits,
                // but belt-and-suspenders against a future change that raises the
                // advertised max.
                let max_frame_size = self.config.max_frame_size() as usize;
                let payload_len = usize::try_from(header.length)
                    .ok()
                    .filter(|n| *n <= max_frame_size && *n <= MAX_BUFFER_SIZE)
                    .ok_or(CloseOutcome::Protocol(H2ErrorCode::FrameSizeError))?;
                let total = FRAME_HEADER_LEN + payload_len;
                self.read_phase = ReadPhase::NeedPayload { total };
                total
            }
            ReadPhase::NeedPayload { total } => total,
        };
        if self.read_filled < total {
            ready!(self.poll_fill_to(total, cx)).map_err(CloseOutcome::Io)?;
        }

        let frame_bytes = &self.read_buf[..total];
        let (frame, consumed) = match Frame::decode(frame_bytes) {
            Ok(pair) => pair,
            Err(FrameDecodeError::Error(code)) => {
                return Poll::Ready(Err(CloseOutcome::Protocol(code)));
            }
            // Unreachable: we read exactly `header.length` payload bytes.
            Err(FrameDecodeError::Incomplete) => {
                return Poll::Ready(Err(CloseOutcome::Protocol(H2ErrorCode::FrameSizeError)));
            }
        };
        let action = self.dispatch(frame, consumed, total)?;
        self.reset_after_frame();
        Poll::Ready(Ok(action))
    }

    /// Read the 24-byte client connection preface (§3.4) and validate it. Uses `read_buf` /
    /// `read_filled` so a partial preface survives a return to `Poll::Pending`.
    pub(super) fn poll_read_preface(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), CloseOutcome>> {
        ready!(self.poll_fill_to(CLIENT_PREFACE.len(), cx)).map_err(CloseOutcome::Io)?;
        if &self.read_buf[..CLIENT_PREFACE.len()] != CLIENT_PREFACE {
            return Poll::Ready(Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError)));
        }
        // Preface is NOT a frame — reset the read cursor so the frame reader starts fresh.
        self.read_buf.clear();
        self.read_buf.resize(FRAME_HEADER_LEN, 0);
        self.read_filled = 0;
        self.read_phase = ReadPhase::NeedHeader;
        Poll::Ready(Ok(()))
    }

    /// Decoded frame arrived — run the connection-level side-effects.
    ///
    /// `payload_start` is the offset within `self.read_buf` where the frame's body bytes
    /// begin (past the fixed header and any per-frame prefix — same value `Frame::decode`
    /// returned). `total` is the full `FRAME_HEADER_LEN + payload_len` so header-block /
    /// data consumers can slice against it.
    fn dispatch(
        &mut self,
        frame: Frame,
        payload_start: usize,
        total: usize,
    ) -> Result<Action, CloseOutcome> {
        // §6.10: while a HEADERS block is in progress (pending_headers.is_some()), the
        // ONLY frame the peer may send on any stream is the matching CONTINUATION.
        // Anything else is a connection-level PROTOCOL_ERROR.
        if self.pending_headers.is_some() && !matches!(frame, Frame::Continuation { .. }) {
            return Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError));
        }

        match frame {
            Frame::Settings(settings) => {
                self.apply_peer_settings(&settings)?;
                self.queue_settings_ack();
                Ok(Action::Continue)
            }
            Frame::Ping {
                opaque_data,
                ack: false,
            } => {
                self.queue_ping_ack(opaque_data);
                Ok(Action::Continue)
            }
            Frame::Goaway { .. } => {
                log::trace!("h2 driver: received peer GOAWAY");
                self.connection.swansong().shut_down();
                Ok(Action::Close(CloseOutcome::Graceful))
            }
            Frame::Headers {
                stream_id,
                end_stream,
                end_headers,
                priority,
                header_block_length,
                ..
            } => self.handle_headers(
                stream_id,
                end_stream,
                end_headers,
                priority,
                header_block_length,
                payload_start,
                total,
            ),
            Frame::Continuation {
                stream_id,
                end_headers,
                header_block_length,
            } => self.handle_continuation(stream_id, end_headers, header_block_length, total),
            Frame::Data {
                stream_id,
                end_stream,
                data_length,
                ..
            } => {
                self.route_data(stream_id, end_stream, data_length, payload_start, total)?;
                Ok(Action::Continue)
            }
            Frame::WindowUpdate {
                stream_id,
                increment,
            } => {
                self.apply_window_update(stream_id, increment)?;
                Ok(Action::Continue)
            }
            Frame::Priority {
                stream_id,
                priority,
            } => {
                self.handle_priority(stream_id, priority);
                Ok(Action::Continue)
            }
            Frame::RstStream { stream_id, .. } => {
                // §5.1: `RST_STREAM` on an idle stream is a connection-level
                // `PROTOCOL_ERROR`; on a closed or active stream it's benign.
                if stream_id > self.last_peer_stream_id && !self.streams.contains_key(&stream_id) {
                    return Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError));
                }
                if let Some(entry) = self.streams.get(&stream_id) {
                    // Unblock any handler task blocked on `poll_read` — the peer has
                    // abandoned this stream so no more request body bytes are coming.
                    // `eof` plus a waker wake is how we tell the recv side "end of data"
                    // in the normal path too.
                    entry.shared.recv.eof.store(true, Ordering::Release);
                    entry.shared.recv.waker.wake();
                    self.complete_and_remove_stream(
                        stream_id,
                        Err(std::io::Error::other("peer RST_STREAM")),
                    );
                } else {
                    // Already closed from our side; still record (idempotent) so later
                    // stray peer frames on this id map to the right error category.
                    self.closed_streams.record(stream_id, ClosedReason::Reset);
                }
                Ok(Action::Continue)
            }
            // §6.6 PUSH_PROMISE from a client is a connection error; §6.10 CONTINUATION
            // without an in-progress header block is too (but pending_headers==Some is
            // handled via the match arm above).
            Frame::PushPromise { .. } => Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError)),
            // PING ACK: complete the matching `H2Connection::send_ping` future, recording
            // the RTT. Unsolicited ACKs (no matching opaque) are silently tolerated per §6.7.
            Frame::Ping {
                opaque_data,
                ack: true,
            } => {
                self.connection.complete_pending_ping(opaque_data);
                Ok(Action::Continue)
            }
            // Informational-only for our current feature set:
            // - `SETTINGS_ACK` (§6.5.3): confirms the peer is using our advertised SETTINGS. We
            //   start enforcing our values immediately on send, not on ack, so there's no deferred
            //   state to apply. We also don't implement `SETTINGS_TIMEOUT` — a peer that never acks
            //   our SETTINGS stays connected.
            // - `Unknown` (§5.5): unknown frame types MUST be ignored.
            Frame::SettingsAck | Frame::Unknown { .. } => Ok(Action::Continue),
        }
    }

    /// §5.3.1 / §6.3: PRIORITY frames on idle streams are allowed (they don't open the
    /// stream but record priority). A PRIORITY frame that names its own stream as its
    /// dependency is a stream-level `PROTOCOL_ERROR`. We don't use the priority info
    /// ourselves — RFC 9113 deprecated the scheme — but we validate for conformance.
    fn handle_priority(&mut self, stream_id: u32, priority: crate::h2::frame::PriorityInfo) {
        if priority.stream_dependency == stream_id {
            self.queue_rst_stream(stream_id, H2ErrorCode::ProtocolError);
        }
    }

    /// A DATA frame arrived — copy its payload into the matching stream's recv buffer and
    /// wake the handler. Padding bytes are part of the already-read frame body and are
    /// skipped (they're in the buffer but not pushed).
    ///
    /// Stream-state errors per RFC 9113 §5.1 / §6.1:
    /// - **Idle** (`stream_id` > `last_peer_stream_id`): DATA on an unopened stream is a
    ///   connection-level `PROTOCOL_ERROR`.
    /// - **Closed** (`stream_id` ≤ `last_peer_stream_id`, not in active map): stream-level
    ///   `RST_STREAM(STREAM_CLOSED)`. Sent after-the-fact — peer has already written this frame and
    ///   we've already read it off the wire.
    /// - **Half-closed remote** (in map, `recv.eof` already set): same stream-level
    ///   `STREAM_CLOSED`.
    ///
    /// Flow-control accounting per RFC 9113 §6.9.1: the entire DATA payload (including
    /// pad length byte + padding) counts against both the per-stream and connection-level
    /// recv windows. We track both for correct refill accounting but enforce leniently —
    /// a peer that sends past our advertised window is simply violating the SETTINGS
    /// hint; the real `DoS` bound is the per-stream buffer cap
    /// (`HttpConfig::h2_max_stream_recv_window_size`).
    /// This keeps trillium's lazy-WU default (`SETTINGS_INITIAL_WINDOW_SIZE = 0`) working
    /// against h2spec-style peers that send DATA immediately after HEADERS without
    /// respecting the server's advertised initial window.
    fn route_data(
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

        // Connection-level accounting runs regardless of stream state (§6.9.1).
        self.connection_recv_window -= flow_controlled;

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
        entry.peer_recv_window -= flow_controlled;
        let state = entry.shared.clone();

        // Half-closed remote: peer already sent END_STREAM on this stream; any DATA after
        // that is stream-level STREAM_CLOSED. Flow-control accounting above still applies.
        if state.recv.eof.load(Ordering::Acquire) {
            self.queue_rst_stream(stream_id, H2ErrorCode::StreamClosed);
            self.complete_and_remove_stream(
                stream_id,
                Err(std::io::Error::other("DATA after END_STREAM on h2 stream")),
            );
            return Ok(());
        }

        let data = frame_slice(&self.read_buf, payload_start, data_length, total)?;

        {
            let mut recv = state.recv.buf.lock().expect("recv buf mutex poisoned");
            // Per-stream buffer cap — this is our actual DoS bound, since
            // `peer_recv_window` is tracked but not strictly enforced. A peer that
            // floods us past the buffer cap earns a connection-level `FLOW_CONTROL_ERROR`.
            if recv.len() + data.len() > self.config.max_stream_recv_window_size() as usize {
                return Err(CloseOutcome::Protocol(H2ErrorCode::FlowControlError));
            }
            if !data.is_empty() {
                recv.extend_from_slice(data);
            }
            if end_stream {
                state.recv.eof.store(true, Ordering::Release);
            }
        }
        state.recv.waker.wake();
        // Client-role lifecycle: peer END_STREAM on the response body might be the second
        // half of "both halves done" — if our send pump has already signaled completion,
        // close the stream now. Server-role removal happens on send completion (via
        // `finalize_send`); recv-side END_STREAM there is informational.
        if end_stream && self.role == Role::Client {
            self.try_close_if_both_done(stream_id);
        }
        Ok(())
    }

    /// Integrate a just-received peer SETTINGS frame into driver state. Only the fields
    /// present (`Some`) in the incoming settings are applied; the rest keep their
    /// previously-negotiated value.
    ///
    /// Per RFC 9113 §6.5.3, all values MUST be processed in order before we ack; because
    /// our applied state is derived from the already-decoded `H2Settings` (which parses
    /// each entry sequentially into its typed fields), that order is preserved for
    /// everything except duplicate ids within the same frame — in which case `H2Settings`
    /// itself keeps only the last value, matching "process in order".
    ///
    /// A change to `INITIAL_WINDOW_SIZE` must be applied as a *delta* (new − previously
    /// effective) to every open stream's send window, per RFC 9113 §6.9.2. The delta can
    /// drive a window negative (legal); it cannot push it past `2^31 − 1` (connection-
    /// level `FLOW_CONTROL_ERROR`).
    fn apply_peer_settings(&mut self, settings: &H2Settings) -> Result<(), CloseOutcome> {
        // Compute INITIAL_WINDOW_SIZE delta against the previously effective value before
        // we take the lock, so the per-stream adjustment below doesn't need to reenter it.
        let initial_window_delta = settings.initial_window_size().map(|new| {
            let old = self
                .connection
                .current_peer_settings()
                .effective_initial_window_size();
            i64::from(new) - i64::from(old)
        });

        // Apply the delta before writing the new settings so a partial failure leaves
        // `peer_settings.initial_window_size` unchanged too — the whole SETTINGS frame
        // is either accepted or it's a connection error.
        if let Some(delta) = initial_window_delta
            && delta != 0
        {
            for entry in self.streams.values_mut() {
                let new = entry.send_window + delta;
                if new > MAX_FLOW_CONTROL_WINDOW {
                    return Err(CloseOutcome::Protocol(H2ErrorCode::FlowControlError));
                }
                entry.send_window = new;
            }
        }

        let mut current = self.connection.current_peer_settings();
        if let Some(v) = settings.max_frame_size() {
            current.set_max_frame_size(Some(v));
        }
        if let Some(v) = settings.initial_window_size() {
            current.set_initial_window_size(Some(v));
        }
        if let Some(v) = settings.max_header_list_size() {
            current.set_max_header_list_size(Some(v));
        }
        if let Some(v) = settings.header_table_size() {
            current.set_header_table_size(Some(v));
        }
        if let Some(v) = settings.enable_push() {
            current.set_enable_push(Some(v));
        }
        if let Some(v) = settings.max_concurrent_streams() {
            current.set_max_concurrent_streams(Some(v));
        }
        if let Some(v) = settings.enable_connect_protocol() {
            current.set_enable_connect_protocol(Some(v));
        }
        // ENABLE_PUSH / MAX_CONCURRENT_STREAMS / HEADER_TABLE_SIZE aren't consulted on the
        // send path today: server-side push is never emitted, the peer's MAX_CONCURRENT_STREAMS
        // applies to peer-initiated streams (we don't initiate), and the static-or-literal
        // HPACK encoder doesn't track the peer's table-size cap. They're stored here
        // regardless so conn-task code that inspects the settings sees a complete picture.
        // ENABLE_CONNECT_PROTOCOL (RFC 8441 §3) is read by client-role conn tasks to gate
        // sending extended CONNECT for WebSocket-over-h2.
        drop(current);
        // Latch + wake any `PeerSettings` futures *after* releasing the mutex so wakers
        // polling immediately don't contend on it. Release ordering on the latch pairs with
        // the Acquire load in `is_resolved_for_peer_settings`.
        self.connection.note_peer_settings();
        Ok(())
    }

    /// Apply a peer `WINDOW_UPDATE`. Connection-level updates (`stream_id == 0`) credit
    /// the driver's `connection_send_window`; stream-level updates credit the matching
    /// `StreamEntry.send_window`.
    ///
    /// RFC 9113 §6.9.1 bounds every flow-control window at `2^31 - 1`. An increment that
    /// would push a window past that maximum is a `FLOW_CONTROL_ERROR`, handled at the
    /// appropriate level:
    /// - Connection window overflow → connection-level GOAWAY (via the returned error).
    /// - Stream window overflow → stream-level `RST_STREAM`, stream cleanup, connection continues.
    ///
    /// A `WINDOW_UPDATE` on a stream we don't know is benign per §6.9 (the peer may send
    /// one after the stream has closed): log and move on.
    fn apply_window_update(&mut self, stream_id: u32, increment: u32) -> Result<(), CloseOutcome> {
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
            // §5.1: WINDOW_UPDATE on an idle stream is a connection error. On a closed
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

    /// Clear read cursor state and prepare for the next frame.
    fn reset_after_frame(&mut self) {
        self.read_filled = 0;
        self.read_phase = ReadPhase::NeedHeader;
        // Shrink if we ballooned above the default capacity for a big frame.
        if self.read_buf.capacity() > MAX_BUFFER_SIZE / 16 {
            self.read_buf = vec![0u8; FRAME_HEADER_LEN];
        } else {
            self.read_buf.truncate(FRAME_HEADER_LEN);
        }
    }
}
