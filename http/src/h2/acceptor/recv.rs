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
mod stream_frames;

use super::{Action, CloseOutcome, H2Driver, MAX_BUFFER_SIZE, MAX_FLOW_CONTROL_WINDOW, ReadPhase};
use crate::{
    Priority,
    h2::{
        H2ErrorCode, H2Settings,
        frame::{FRAME_HEADER_LEN, Frame, FrameDecodeError, FrameHeader},
        role::Role,
    },
};
use futures_lite::io::{AsyncRead, AsyncWrite};
pub(super) use headers::PendingHeaders;
use std::task::{Context, Poll, ready};

/// The client connection preface — 24 bytes the client MUST send before any HTTP/2
/// frames.
pub(super) const CLIENT_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// Upper bound on per-stream `PRIORITY_UPDATE` signals retained at once. Updates may
/// arrive before the stream they target opens, so the table can't be tied to the active
/// stream set; this bounds it against a peer that floods updates for streams it never
/// opens.
pub(super) const MAX_TRACKED_PRIORITIES: usize = 128;

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
                let header = FrameHeader::decode(&self.read_buf[self.read_start..])
                    .expect("FRAME_HEADER_LEN bytes already filled");
                // A frame whose length exceeds our advertised `SETTINGS_MAX_FRAME_SIZE`
                // is a `FRAME_SIZE_ERROR`. We also enforce [`MAX_BUFFER_SIZE`] as a DoS
                // guard — belt-and-suspenders against a future change that raises the
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
        ready!(self.poll_fill_to(total, cx)).map_err(CloseOutcome::Io)?;

        let frame_bytes = &self.read_buf[self.read_start..self.read_start + total];
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
        self.consume_read(total);
        Poll::Ready(Ok(action))
    }

    /// Read the 24-byte client connection preface and validate it. Reads through `read_buf`, so
    /// a partial preface survives a return to `Poll::Pending`.
    pub(super) fn poll_read_preface(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), CloseOutcome>> {
        ready!(self.poll_fill_to(CLIENT_PREFACE.len(), cx)).map_err(CloseOutcome::Io)?;
        let preface_end = self.read_start + CLIENT_PREFACE.len();
        if &self.read_buf[self.read_start..preface_end] != CLIENT_PREFACE {
            return Poll::Ready(Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError)));
        }
        // The preface is not a frame; any bytes read past it are the first frames.
        self.consume_read(CLIENT_PREFACE.len());
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
        log_received_frame(&frame);
        // While a HEADERS block is in progress (pending_headers.is_some()), the ONLY
        // frame the peer may send on any stream is the matching CONTINUATION. Anything
        // else is a connection-level PROTOCOL_ERROR.
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
            Frame::PriorityUpdate {
                prioritized_stream_id,
                priority,
            } => {
                self.handle_priority_update(prioritized_stream_id, priority);
                Ok(Action::Continue)
            }
            Frame::RstStream {
                stream_id,
                error_code,
            } => {
                self.handle_rst_stream(stream_id, error_code)?;
                Ok(Action::Continue)
            }
            // PUSH_PROMISE from a client is a connection error; bare CONTINUATION without
            // an in-progress header block is too (but pending_headers==Some is handled via
            // the match arm above).
            Frame::PushPromise { .. } => Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError)),
            // PING ACK: complete the matching `H2Connection::send_ping` future, recording
            // the RTT. Unsolicited ACKs (no matching opaque) are silently tolerated.
            Frame::Ping {
                opaque_data,
                ack: true,
            } => {
                self.connection.complete_pending_ping(opaque_data);
                Ok(Action::Continue)
            }
            // Informational-only for our current feature set:
            // - `SETTINGS_ACK`: confirms the peer is using our advertised SETTINGS. We start
            //   enforcing our values immediately on send, not on ack, so there's no deferred state
            //   to apply. We also don't implement `SETTINGS_TIMEOUT` — a peer that never acks our
            //   SETTINGS stays connected.
            // - `Unknown`: unknown frame types MUST be ignored.
            Frame::SettingsAck | Frame::Unknown { .. } => Ok(Action::Continue),
        }
    }

    /// PRIORITY frames on idle streams are allowed (they don't open the stream but record
    /// priority). A PRIORITY frame that names its own stream as its dependency is a
    /// stream-level `PROTOCOL_ERROR`. We don't use the priority info ourselves — the spec
    /// deprecated the scheme — but we validate for conformance.
    fn handle_priority(&mut self, stream_id: u32, priority: crate::h2::frame::PriorityInfo) {
        if priority.stream_dependency == stream_id {
            self.queue_rst_stream(stream_id, H2ErrorCode::ProtocolError);
        }
    }

    /// Record an RFC 9218 `PRIORITY_UPDATE` for later scheduling. The signal is advisory:
    /// the latest value per stream wins, and an update that targets a stream not yet opened
    /// is retained until its HEADERS arrive.
    ///
    /// Only the server role acts on these — a client emits them to prioritize the responses
    /// it requested, so a client-role driver has nothing to schedule. The prioritized stream
    /// id must name a client-initiated (odd, nonzero) request stream; anything else is
    /// ignored rather than treated as an error, since priority never costs the connection.
    fn handle_priority_update(&mut self, prioritized_stream_id: u32, priority: Priority) {
        if self.role != Role::Server
            || prioritized_stream_id == 0
            || prioritized_stream_id.is_multiple_of(2)
        {
            return;
        }
        let at_capacity = self.stream_priorities.len() >= MAX_TRACKED_PRIORITIES;
        if at_capacity && !self.stream_priorities.contains_key(&prioritized_stream_id) {
            log::trace!(
                "h2 stream {prioritized_stream_id}: dropping PRIORITY_UPDATE \"{priority}\" \
                 (tracking table full)"
            );
            return;
        }
        log::trace!("h2 stream {prioritized_stream_id}: received PRIORITY_UPDATE \"{priority}\"");
        self.stream_priorities
            .insert(prioritized_stream_id, priority);
    }

    /// Integrate a just-received peer SETTINGS frame into driver state. Only the fields
    /// present (`Some`) in the incoming settings are applied; the rest keep their
    /// previously-negotiated value.
    ///
    /// All values MUST be processed in order before we ack; because our applied state is
    /// derived from the already-decoded `H2Settings` (which parses each entry sequentially
    /// into its typed fields), that order is preserved for everything except duplicate ids
    /// within the same frame — in which case `H2Settings` itself keeps only the last
    /// value, matching "process in order".
    ///
    /// A change to `INITIAL_WINDOW_SIZE` must be applied as a *delta* (new − previously
    /// effective) to every open stream's send window. The delta can drive a window
    /// negative (legal); it cannot push it past `2^31 − 1` (connection-level
    /// `FLOW_CONTROL_ERROR`).
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

        // LOAD-BEARING ORDERING: apply the per-stream delta BEFORE mutating
        // `peer_settings.initial_window_size`. A `FlowControlError` partway through
        // (the only failure mode here) must leave the stored setting consistent with
        // the per-stream `send_window`s we actually applied — otherwise a later
        // `effective_initial_window_size()` read would compute the wrong delta
        // against the *next* SETTINGS frame. SETTINGS frames are atomic: accepted whole
        // or treated as a connection error. Do not reorder without preserving that
        // invariant.
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
        // ENABLE_PUSH / MAX_CONCURRENT_STREAMS aren't consulted on the send path: server-side
        // push is never emitted, and the peer's MAX_CONCURRENT_STREAMS applies to
        // peer-initiated streams (we don't initiate). They're stored here regardless so
        // conn-task code that inspects the settings sees a complete picture.
        // ENABLE_CONNECT_PROTOCOL (RFC 8441) is read by client-role conn tasks to gate
        // sending extended CONNECT for WebSocket-over-h2.
        drop(current);
        // Apply peer's HEADER_TABLE_SIZE to the HPACK encoder. The encoder caps its
        // operational size at `min(local_preferred, peer_advertised)`; a change queues a
        // Dynamic Table Size Update for emission on the next encode (per RFC 7541).
        if let Some(v) = settings.header_table_size() {
            self.hpack_encoder
                .set_protocol_max_size(usize::try_from(v).unwrap_or(usize::MAX));
        }
        // Latch + wake any `PeerSettings` futures *after* releasing the mutex so wakers
        // polling immediately don't contend on it. Release ordering on the latch pairs with
        // the Acquire load in `is_resolved_for_peer_settings`.
        self.connection.note_peer_settings();
        Ok(())
    }
}

/// One-line summary of a just-received frame: type, stream, salient flags / values.
/// Mirrors the `h2 emit: …` log shape in [`super::outbound`] so an inbound + outbound
/// interleaved trace reads as a clean narrative of the wire.
fn log_received_frame(frame: &Frame) {
    if !log::log_enabled!(log::Level::Trace) {
        return;
    }
    match frame {
        Frame::Data {
            stream_id,
            end_stream,
            data_length,
            ..
        } => log::trace!(
            "h2 recv: DATA stream={stream_id} len={data_length} end_stream={end_stream}",
        ),
        Frame::Headers {
            stream_id,
            end_stream,
            end_headers,
            header_block_length,
            ..
        } => log::trace!(
            "h2 recv: HEADERS stream={stream_id} block_len={header_block_length} \
             end_headers={end_headers} end_stream={end_stream}",
        ),
        Frame::Continuation {
            stream_id,
            end_headers,
            header_block_length,
        } => log::trace!(
            "h2 recv: CONTINUATION stream={stream_id} block_len={header_block_length} \
             end_headers={end_headers}",
        ),
        Frame::Settings(s) => log::trace!("h2 recv: SETTINGS {s:?}"),
        Frame::SettingsAck => log::trace!("h2 recv: SETTINGS ACK"),
        Frame::Ping { opaque_data, ack } => {
            log::trace!("h2 recv: PING opaque={opaque_data:?} ack={ack}");
        }
        Frame::Goaway {
            last_stream_id,
            error_code,
            ..
        } => log::trace!("h2 recv: GOAWAY last_stream_id={last_stream_id} code={error_code:?}"),
        Frame::WindowUpdate {
            stream_id,
            increment,
        } => log::trace!("h2 recv: WINDOW_UPDATE stream={stream_id} increment={increment}"),
        Frame::RstStream {
            stream_id,
            error_code,
        } => log::trace!("h2 recv: RST_STREAM stream={stream_id} code={error_code:?}"),
        Frame::Priority {
            stream_id,
            priority,
        } => log::trace!(
            "h2 recv: PRIORITY stream={stream_id} dep={} excl={} weight={}",
            priority.stream_dependency,
            priority.exclusive,
            priority.weight,
        ),
        Frame::PriorityUpdate {
            prioritized_stream_id,
            priority,
        } => log::trace!(
            "h2 recv: PRIORITY_UPDATE stream={prioritized_stream_id} priority=\"{priority}\"",
        ),
        Frame::PushPromise { stream_id, length } => {
            log::trace!("h2 recv: PUSH_PROMISE stream={stream_id} length={length}");
        }
        Frame::Unknown {
            stream_id,
            frame_type,
            flags,
            length,
        } => log::trace!(
            "h2 recv: UNKNOWN type={frame_type} stream={stream_id} flags={flags:#04x} \
             length={length}",
        ),
    }
}
