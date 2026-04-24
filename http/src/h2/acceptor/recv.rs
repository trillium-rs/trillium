//! Receive side of the HTTP/2 driver: frame reading, dispatch, HEADERS+CONTINUATION
//! accumulation, malformed-request `RST_STREAM`, DATA routing into per-stream recv rings.
//!
//! All methods are on [`super::H2Acceptor`] — split off here to keep the driver's send and
//! receive logic in separate files. Visibility-wise, this child module reaches up via
//! `super::*` for everything it needs from the parent.

use super::{
    Action, CloseOutcome, H2Acceptor, MAX_BUFFER_SIZE, MAX_FLOW_CONTROL_WINDOW, ReadPhase,
    StreamEntry, frame_slice, io_to_outcome,
};
use crate::{
    Conn,
    h2::{
        H2Error, H2ErrorCode, H2Settings,
        frame::{FRAME_HEADER_LEN, Frame, FrameDecodeError, FrameHeader},
        transport::{H2Transport, StreamState},
    },
};
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    sync::{Arc, atomic::Ordering},
    task::{Context, Poll, ready},
};

/// The client connection preface (RFC 9113 §3.4). 24 bytes the client MUST send before any
/// HTTP/2 frames.
const CLIENT_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// HEADERS + CONTINUATION assembly state.
#[derive(Debug)]
pub(super) struct PendingHeaders {
    stream_id: u32,
    end_stream: bool,
    assembled: Vec<u8>,
}

impl<T> H2Acceptor<T>
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
        if matches!(self.read_phase, ReadPhase::NeedHeader) {
            ready!(self.poll_fill_to(FRAME_HEADER_LEN, cx)).map_err(io_to_outcome)?;
            let header = FrameHeader::decode(&self.read_buf[..FRAME_HEADER_LEN])
                .expect("FRAME_HEADER_LEN bytes already filled");
            let payload_len = usize::try_from(header.length)
                .ok()
                .filter(|n| *n <= MAX_BUFFER_SIZE)
                .ok_or(CloseOutcome::Protocol(H2ErrorCode::FrameSizeError))?;
            let total = FRAME_HEADER_LEN + payload_len;
            self.read_phase = ReadPhase::NeedPayload { header, total };
        }

        let ReadPhase::NeedPayload { total, .. } = self.read_phase else {
            unreachable!("set by the block above")
        };
        if self.read_filled < total {
            ready!(self.poll_fill_to(total, cx)).map_err(io_to_outcome)?;
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
        ready!(self.poll_fill_to(CLIENT_PREFACE.len(), cx)).map_err(io_to_outcome)?;
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
                self.apply_peer_settings(&settings);
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
                self.connection.swansong().shut_down();
                Ok(Action::Close(CloseOutcome::Graceful))
            }
            Frame::Headers {
                stream_id,
                end_stream,
                end_headers,
                header_block_length,
                ..
            } => self.handle_headers(
                stream_id,
                end_stream,
                end_headers,
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
                padding_length,
            } => {
                self.route_data(
                    stream_id,
                    end_stream,
                    data_length,
                    padding_length,
                    payload_start,
                    total,
                )?;
                Ok(Action::Continue)
            }
            Frame::WindowUpdate {
                stream_id,
                increment,
            } => {
                self.apply_window_update(stream_id, increment)?;
                Ok(Action::Continue)
            }
            // §6.6 PUSH_PROMISE from a client is a connection error; §6.10 CONTINUATION
            // without an in-progress header block is too (but pending_headers==Some is
            // handled via the match arm above).
            Frame::PushPromise { .. } => Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError)),
            // Benign frames whose effect isn't yet implemented. Tolerate to keep the
            // handshake clean until the relevant phases.
            Frame::SettingsAck
            | Frame::Ping { ack: true, .. }
            | Frame::RstStream { .. }
            | Frame::Priority { .. }
            | Frame::Unknown { .. } => Ok(Action::Continue),
        }
    }

    /// A HEADERS frame arrived. Either `END_HEADERS` is set (emit the stream immediately) or
    /// we accumulate the fragment into `pending_headers` and wait for CONTINUATION.
    fn handle_headers(
        &mut self,
        stream_id: u32,
        end_stream: bool,
        end_headers: bool,
        header_block_length: u32,
        payload_start: usize,
        total: usize,
    ) -> Result<Action, CloseOutcome> {
        // §5.1.1: a peer-initiated stream id must be odd and strictly greater than every
        // prior peer-initiated stream id, and not already known.
        if stream_id.is_multiple_of(2)
            || stream_id <= self.last_peer_stream_id
            || self.streams.contains_key(&stream_id)
        {
            return Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError));
        }

        let fragment = frame_slice(&self.read_buf, payload_start, header_block_length, total)?;

        if end_headers {
            let block = fragment.to_vec();
            self.finalize_headers(stream_id, end_stream, &block)
        } else {
            self.pending_headers = Some(PendingHeaders {
                stream_id,
                end_stream,
                assembled: fragment.to_vec(),
            });
            Ok(Action::Continue)
        }
    }

    /// A CONTINUATION frame arrived. Must match the in-progress HEADERS block's stream id.
    fn handle_continuation(
        &mut self,
        stream_id: u32,
        end_headers: bool,
        header_block_length: u32,
        total: usize,
    ) -> Result<Action, CloseOutcome> {
        let pending = self
            .pending_headers
            .as_mut()
            .ok_or(CloseOutcome::Protocol(H2ErrorCode::ProtocolError))?;
        if pending.stream_id != stream_id {
            return Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError));
        }

        let fragment = frame_slice(&self.read_buf, FRAME_HEADER_LEN, header_block_length, total)?;
        pending.assembled.extend_from_slice(fragment);

        if end_headers {
            let PendingHeaders {
                stream_id,
                end_stream,
                assembled,
            } = self.pending_headers.take().expect("checked above");
            self.finalize_headers(stream_id, end_stream, &assembled)
        } else {
            Ok(Action::Continue)
        }
    }

    /// The complete header block is now available (whether from a single HEADERS or from
    /// HEADERS + CONTINUATION*): HPACK-decode it, open the stream, validate the request via
    /// [`Conn::new_h2`], and emit the [`Conn`] on success.
    ///
    /// On a §8.1.2 malformed-request rejection from `new_h2`, the stream is removed from
    /// both maps, a `RST_STREAM(PROTOCOL_ERROR)` is queued, and `Action::Continue` is
    /// returned — the malformed request never reaches a handler task. (HPACK decode
    /// failures, by contrast, are connection-level: the dynamic table state is now
    /// untrustworthy for *every* future stream on this connection.)
    fn finalize_headers(
        &mut self,
        stream_id: u32,
        end_stream: bool,
        block: &[u8],
    ) -> Result<Action, CloseOutcome> {
        let field_section = self.hpack.decode(block).map_err(|e| match e.into() {
            H2Error::Protocol(code) => CloseOutcome::Protocol(code),
            H2Error::Io(e) => CloseOutcome::Io(e),
        })?;

        let state = Arc::new(StreamState::default());
        if end_stream {
            let _guard = state.recv.buf.lock().expect("recv buf mutex poisoned");
            state.recv.eof.store(true, Ordering::Release);
        }
        let send_window = i64::from(
            self.connection
                .peer_settings()
                .effective_initial_window_size(),
        );
        self.connection
            .streams_lock()
            .insert(stream_id, state.clone());
        self.streams
            .insert(stream_id, StreamEntry::new(state.clone(), send_window));
        self.last_peer_stream_id = stream_id;

        // No eager WINDOW_UPDATE: we advertise `INITIAL_WINDOW_SIZE = 0` in SETTINGS, so
        // the peer cannot send body bytes until the handler calls `H2Transport::poll_read`
        // and the driver observes `recv.is_reading` on a subsequent poll.

        let transport = H2Transport::new(self.connection.clone(), stream_id, state);
        match Conn::new_h2(self.connection.clone(), stream_id, field_section, transport) {
            Ok(conn) => Ok(Action::Emit(Box::new(conn))),
            Err(code) => {
                log::debug!("h2 stream {stream_id}: rejected during build: {code:?}");
                self.streams.remove(&stream_id);
                self.connection.streams_lock().remove(&stream_id);
                self.queue_rst_stream(stream_id, code);
                Ok(Action::Continue)
            }
        }
    }

    /// A DATA frame arrived — copy its payload into the matching stream's recv buffer and
    /// wake the handler. Padding bytes are part of the already-read frame body and are
    /// skipped (they're in the buffer but not pushed).
    fn route_data(
        &mut self,
        stream_id: u32,
        end_stream: bool,
        data_length: u32,
        padding_length: u8,
        payload_start: usize,
        total: usize,
    ) -> Result<(), CloseOutcome> {
        let _ = padding_length; // padding is skipped — we only push the data portion

        let entry = self
            .streams
            .get(&stream_id)
            .ok_or(CloseOutcome::Protocol(H2ErrorCode::StreamClosed))?;
        let state = &entry.shared;

        let data = frame_slice(&self.read_buf, payload_start, data_length, total)?;

        {
            let mut recv = state.recv.buf.lock().expect("recv buf mutex poisoned");
            if !data.is_empty() {
                recv.extend_from_slice(data);
            }
            if end_stream {
                state.recv.eof.store(true, Ordering::Release);
            }
        }
        state.recv.waker.wake();
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
    /// Mid-connection `INITIAL_WINDOW_SIZE` changes must be applied as a *delta* to every
    /// open stream's current send window (§6.9.2). That redistribution lands with the
    /// per-stream send windows in a later commit.
    fn apply_peer_settings(&mut self, settings: &H2Settings) {
        let mut current = self.connection.peer_settings_mut();
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
        // ENABLE_PUSH / MAX_CONCURRENT_STREAMS / HEADER_TABLE_SIZE aren't consulted on the
        // send path today: server-side push is never emitted, the peer's MAX_CONCURRENT_STREAMS
        // applies to peer-initiated streams (we don't initiate), and the static-or-literal
        // HPACK encoder doesn't track the peer's table-size cap. They're stored here
        // regardless so conn-task code that inspects the settings sees a complete picture.
    }

    /// Apply a peer `WINDOW_UPDATE`. Connection-level updates (`stream_id == 0`) credit
    /// the driver's `connection_send_window`; stream-level updates credit the matching
    /// `StreamEntry.send_window`.
    ///
    /// RFC 9113 §6.9.1 bounds every flow-control window at `2^31 - 1`. An increment that
    /// would push a window past that maximum is a `FLOW_CONTROL_ERROR`, handled at the
    /// appropriate level:
    /// - Connection window overflow → connection-level GOAWAY (via the returned error).
    /// - Stream window overflow → stream-level `RST_STREAM`, stream cleanup, connection
    ///   continues.
    ///
    /// A `WINDOW_UPDATE` on a stream we don't know is benign per §6.9 (the peer may send
    /// one after the stream has closed): log and move on.
    fn apply_window_update(
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
            log::trace!("WINDOW_UPDATE on unknown stream {stream_id} — ignoring");
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
