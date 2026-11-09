//! HEADERS + CONTINUATION block accumulation and finalization.
//!
//! `handle_headers` and `handle_continuation` are the per-frame entry points called from the
//! parent `recv::dispatch`. The HEADERS frame decides where the block goes; once `END_HEADERS` is
//! observed, [`Self::finalize_headers`] HPACK-decodes the assembled block and acts on that:
//! - **Rejected** (late, refused, or self-dependent stream) → decoded for its HPACK side effects,
//!   then the stream is reset.
//! - **New peer-initiated stream (server role)** → [`Self::finalize_new_request_stream`] opens the
//!   stream, validates the request, and emits a [`Conn`] for the handler task.
//! - **Existing stream, client role, no first response yet** → [`Self::finalize_response_headers`]
//!   stashes the response HEADERS for the conn task.
//! - **Existing stream, anything else** → [`Self::finalize_trailers`] stashes trailers and signals
//!   EOF.
//!
//! All methods are on [`super::super::H2Driver`].

use crate::{
    Conn, KnownHeaderName, Priority, Status,
    h2::{
        H2Error, H2ErrorCode,
        acceptor::{
            Action, CloseOutcome, ClosedReason, H2Driver, Role, StreamEntry, frame_slice,
            inflow::Inflow,
        },
        frame::{FRAME_HEADER_LEN, PriorityInfo},
        stream_state::StreamEvent,
        transport::{H2Transport, StreamState},
    },
    headers::hpack::HpackDecodeError,
};
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    io::Error,
    sync::{Arc, atomic::Ordering},
    time::Instant,
};

/// HEADERS + CONTINUATION assembly state.
#[derive(Debug)]
pub(in crate::h2::acceptor) struct PendingHeaders {
    pub(super) stream_id: u32,
    pub(super) end_stream: bool,
    pub(super) target: HeaderBlockTarget,
    pub(super) assembled: Vec<u8>,
}

/// Where a header block goes. Decided when its HEADERS frame arrives: stream-state transitions
/// belong to the HEADERS frame, and its CONTINUATION frames are part of it.
#[derive(Debug, Clone, Copy)]
pub(super) enum HeaderBlockTarget {
    /// An open stream: the response (client role) or trailers.
    Existing,
    /// A new peer-initiated stream (server role).
    NewRequest,
    /// Reset the stream with this code once the block is decoded.
    Rejected(H2ErrorCode),
}

impl<T> H2Driver<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    /// A HEADERS frame arrived. Classify the block it begins, then either finalize it
    /// (`END_HEADERS` set) or accumulate the fragment into `pending_headers` and wait for
    /// CONTINUATION.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn handle_headers(
        &mut self,
        stream_id: u32,
        end_stream: bool,
        end_headers: bool,
        priority: Option<PriorityInfo>,
        header_block_length: u32,
        payload_start: usize,
        total: usize,
    ) -> Result<Action, CloseOutcome> {
        // Push is disabled, so HEADERS can only name a client-initiated (odd) stream.
        if stream_id.is_multiple_of(2) {
            return Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError));
        }
        let target = self.classify_header_block(stream_id, priority)?;

        let fragment = frame_slice(
            &self.read_buf[self.read_start..],
            payload_start,
            header_block_length,
            total,
        )?;

        if end_headers {
            let block = fragment.to_vec();
            self.finalize_headers(stream_id, end_stream, target, &block)
        } else {
            if fragment.len() as u64 > self.config.max_header_list_size() {
                return Err(CloseOutcome::Protocol(H2ErrorCode::EnhanceYourCalm));
            }
            self.pending_headers = Some(PendingHeaders {
                stream_id,
                end_stream,
                target,
                assembled: fragment.to_vec(),
            });
            Ok(Action::Continue)
        }
    }

    /// Decide where a header block goes, or fail the connection.
    ///
    /// A block for a stream that is absent but not idle arrived after the stream closed. A client
    /// opened every such stream itself, so it rejects the block with stream-level `STREAM_CLOSED`.
    /// A server splits three ways on the closed-streams ledger:
    /// - Closed via `RST_STREAM` (either direction) → stream-level `STREAM_CLOSED`.
    /// - Closed via `END_STREAM` on both sides → connection-level `STREAM_CLOSED`.
    /// - Never opened (implicitly closed by a higher-id HEADERS, or evicted from the bounded
    ///   ledger) → connection-level `PROTOCOL_ERROR` per the spec's "stream identifiers MUST be
    ///   numerically greater than all streams the initiating endpoint has opened".
    fn classify_header_block(
        &mut self,
        stream_id: u32,
        priority: Option<PriorityInfo>,
    ) -> Result<HeaderBlockTarget, CloseOutcome> {
        let is_new_stream = !self.streams.contains_key(&stream_id);
        if is_new_stream && !self.stream_is_idle(stream_id) {
            return match (self.role, self.closed_reason(stream_id)) {
                (Role::Client, _) | (Role::Server, Some(ClosedReason::Reset)) => {
                    Ok(HeaderBlockTarget::Rejected(H2ErrorCode::StreamClosed))
                }
                (Role::Server, Some(ClosedReason::EndStream)) => {
                    Err(CloseOutcome::Protocol(H2ErrorCode::StreamClosed))
                }
                (Role::Server, None) => Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError)),
            };
        }
        // Servers never open odd streams, so an idle one is never legitimately named to a client.
        if is_new_stream && self.role == Role::Client {
            return Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError));
        }

        // A stream cannot depend on itself. Stream-level PROTOCOL_ERROR — the connection
        // stays alive; just RST this stream and advance past it.
        if priority.is_some_and(|p| p.stream_dependency == stream_id) {
            log::debug!("h2 stream {stream_id}: HEADERS depends on itself; RST_STREAM");
            // Advance `last_peer_stream_id` so the peer can't reuse this id.
            if is_new_stream {
                self.last_peer_stream_id = stream_id;
            }
            return Ok(HeaderBlockTarget::Rejected(H2ErrorCode::ProtocolError));
        }

        if is_new_stream {
            // Peer-initiated streams beyond our advertised `SETTINGS_MAX_CONCURRENT_STREAMS`
            // get `RST_STREAM(RefusedStream)`. The identifier is still "consumed" ("The
            // identifier of a refused stream is not reused") so bump `last_peer_stream_id`.
            let max_concurrent = self.config.max_concurrent_streams() as usize;
            if self.streams.len() >= max_concurrent {
                log::debug!(
                    "h2 stream {stream_id}: concurrent stream limit reached ({max_concurrent})"
                );
                self.last_peer_stream_id = stream_id;
                return Ok(HeaderBlockTarget::Rejected(H2ErrorCode::RefusedStream));
            }
            return Ok(HeaderBlockTarget::NewRequest);
        }

        // Half-closed (remote): once we've observed the peer's `END_STREAM`, any further HEADERS
        // on that stream is a stream-level `STREAM_CLOSED`. Trailers arrive while the stream is
        // still recv-open and close it as part of their own transition, so this picks out "second
        // HEADERS after recv-close", not the trailer itself. Role-agnostic: applies to server
        // (peer-sent EOS) and client (peer-sent response EOS) alike.
        if self.streams[&stream_id]
            .shared
            .lifecycle_lock()
            .recv_closed()
        {
            log::debug!("h2 stream {stream_id}: HEADERS on half-closed-remote stream");
            return Ok(HeaderBlockTarget::Rejected(H2ErrorCode::StreamClosed));
        }
        Ok(HeaderBlockTarget::Existing)
    }

    /// Reset a stream whose header block was rejected, tearing it down if it was open.
    fn reject_header_block(&mut self, stream_id: u32, code: H2ErrorCode) {
        self.queue_rst_stream(stream_id, code);
        if self.streams.contains_key(&stream_id) {
            self.complete_and_remove_stream(
                stream_id,
                Err(Error::other(format!("h2 header block rejected: {code:?}"))),
            );
        }
    }

    /// A CONTINUATION frame arrived. Must match the in-progress HEADERS block's stream id.
    pub(super) fn handle_continuation(
        &mut self,
        stream_id: u32,
        end_headers: bool,
        header_block_length: u32,
        total: usize,
    ) -> Result<Action, CloseOutcome> {
        let max_block = self.config.max_header_list_size();
        let pending = self
            .pending_headers
            .as_mut()
            .ok_or(CloseOutcome::Protocol(H2ErrorCode::ProtocolError))?;
        if pending.stream_id != stream_id {
            return Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError));
        }

        let fragment = frame_slice(
            &self.read_buf[self.read_start..],
            FRAME_HEADER_LEN,
            header_block_length,
            total,
        )?;
        // CONTINUATION-flood guard (CVE-2024-27316 class): a peer that never sets END_HEADERS
        // can grow this block without bound. Cap the accumulated *compressed* bytes at the
        // MAX_HEADER_LIST_SIZE we advertised — a compliant peer's wire bytes never exceed that
        // uncompressed budget, so overflow is abuse. Reject before decoding so junk can't OOM us.
        if pending.assembled.len() as u64 + fragment.len() as u64 > max_block {
            return Err(CloseOutcome::Protocol(H2ErrorCode::EnhanceYourCalm));
        }
        pending.assembled.extend_from_slice(fragment);

        if end_headers {
            let PendingHeaders {
                stream_id,
                end_stream,
                target,
                assembled,
            } = self.pending_headers.take().expect("checked above");
            self.finalize_headers(stream_id, end_stream, target, &assembled)
        } else {
            Ok(Action::Continue)
        }
    }

    /// The complete header block is now available (whether from a single HEADERS or from
    /// HEADERS + CONTINUATION*). HPACK-decodes it — always, since even a rejected block may
    /// update the connection-wide dynamic table — then acts on its `target`:
    /// - **Rejected:** reset the stream, tearing it down if it was open.
    /// - **New request:** open the stream, validate the request via [`Conn::new_h2`], and emit the
    ///   [`Conn`] on success; on a malformed-request rejection, queue `RST_STREAM(PROTOCOL_ERROR)`
    ///   and drop the stream before a handler task ever sees it.
    /// - **Existing stream:** the response HEADERS (client role) or trailers. A trailer violation
    ///   queues `RST_STREAM(PROTOCOL_ERROR)` on the offending stream and leaves the connection
    ///   open.
    ///
    /// HPACK decode failures split by variant: wire-format compression errors are
    /// connection-level (dynamic table now untrusted for every future stream — bubble up as
    /// `CloseOutcome::Protocol`), while spec-defined request malformation is stream-level
    /// (`RST_STREAM(PROTOCOL_ERROR)` and the connection continues).
    fn finalize_headers(
        &mut self,
        stream_id: u32,
        end_stream: bool,
        target: HeaderBlockTarget,
        block: &[u8],
    ) -> Result<Action, CloseOutcome> {
        let decoded = match self.hpack.decode(block) {
            Ok(field_section) => Ok(field_section),
            Err(HpackDecodeError::Compression(e)) => {
                let h2err: H2Error = e.into();
                return Err(match h2err {
                    H2Error::Protocol(code) => CloseOutcome::Protocol(code),
                    H2Error::Io(e) => CloseOutcome::Io(e),
                });
            }
            Err(HpackDecodeError::MalformedRequest(reason)) => {
                log::debug!("h2 stream {stream_id}: malformed request headers: {reason:?}");
                Err(H2ErrorCode::ProtocolError)
            }
        };

        match (target, decoded) {
            // Reset or canceled locally between its HEADERS frame and END_HEADERS; the peer sent
            // the block before it could know. Decoding was all it needed.
            (HeaderBlockTarget::Existing, _) if !self.streams.contains_key(&stream_id) => {
                log::trace!("h2 stream {stream_id}: dropping header block for a closed stream");
            }
            (HeaderBlockTarget::Rejected(code), _) | (_, Err(code)) => {
                self.reject_header_block(stream_id, code);
            }
            (HeaderBlockTarget::NewRequest, Ok(field_section)) => {
                return Ok(self.finalize_new_request_stream(stream_id, end_stream, field_section));
            }
            (HeaderBlockTarget::Existing, Ok(field_section)) => {
                self.finalize_existing_stream_headers(stream_id, end_stream, field_section);
            }
        }
        Ok(Action::Continue)
    }

    /// HEADERS on an open stream. Role-asymmetric: a server treats it as trailers; a client's
    /// first arrival is the response headers and any later one is trailers.
    fn finalize_existing_stream_headers(
        &mut self,
        stream_id: u32,
        end_stream: bool,
        field_section: crate::headers::hpack::FieldSection<'static>,
    ) {
        match self.role {
            Role::Server => self.finalize_trailers(stream_id, end_stream, field_section),
            Role::Client => {
                // Latching flag, not slot occupancy: the conn task drains `response_headers` when
                // it consumes them, so the slot would falsely read empty for a trailing-HEADERS
                // arriving after the conn task has taken the response.
                let already_have_response = self
                    .streams
                    .get(&stream_id)
                    .expect("caller verified stream is present")
                    .shared
                    .recv
                    .first_response_headers_seen
                    .load(Ordering::Acquire);
                if already_have_response {
                    // Second HEADERS arrival on a client-initiated stream is the trailing
                    // HEADERS — same constraints as the server-side trailer path (no pseudos,
                    // MUST carry END_STREAM).
                    self.finalize_trailers(stream_id, end_stream, field_section);
                } else {
                    self.finalize_response_headers(stream_id, end_stream, field_section);
                }
            }
        }
    }

    /// Client-role: first HEADERS arrival on a client-initiated stream is the response
    /// HEADERS — stash on `StreamState.recv.response_headers`, fire the response-headers
    /// waker, and (if `END_STREAM` is set) flip recv-side eof and try to close the stream
    /// if our send half has already completed.
    ///
    /// Interim (1xx) HEADERS frames are discarded: the response may include zero or more
    /// informational HEADERS frames before the final, and their headers must not be merged
    /// into the final response. Discarding without latching `first_response_headers_seen`
    /// routes the next HEADERS arrival through this function as the final response.
    ///
    /// Validation of pseudo-headers (e.g. presence of `:status`) is left to the conn task.
    fn finalize_response_headers(
        &mut self,
        stream_id: u32,
        end_stream: bool,
        field_section: crate::headers::hpack::FieldSection<'static>,
    ) {
        let status = field_section.pseudo_headers().status();
        if status.is_some_and(|s| s.is_informational() && s != Status::SwitchingProtocols) {
            log::trace!("h2 stream {stream_id}: discarding interim response {status:?}");
            if end_stream {
                // The spec forbids END_STREAM on an interim HEADERS frame. Honor it
                // anyway so the conn task surfaces `ConnectionAborted` rather than hanging
                // on a final-response HEADERS frame that won't arrive.
                let state = self
                    .streams
                    .get(&stream_id)
                    .expect("caller verified stream is present")
                    .shared
                    .clone();
                let _ = state.apply_event(StreamEvent::RecvHeaders { end_stream: true });
                state.recv.response_headers_waker.wake();
                state.recv.waker.wake();
                self.close_if_both_done(stream_id);
            }
            return;
        }

        // A present Content-Length must be a single run of digits. The client conn
        // task validates response pseudo-headers, but not this — and only the driver can answer a
        // malformed framing header with the spec-mandated `RST_STREAM(PROTOCOL_ERROR)`, so the
        // check lives here rather than in the conn task.
        if crate::util::validate_content_length(
            field_section
                .headers()
                .get_values(KnownHeaderName::ContentLength),
        )
        .is_err()
        {
            log::debug!("h2 stream {stream_id}: malformed response content-length");
            self.queue_rst_stream(stream_id, H2ErrorCode::ProtocolError);
            self.complete_and_remove_stream(
                stream_id,
                Err(Error::other("malformed h2 response content-length")),
            );
            return;
        }

        // Declared response-body length, for the content-length / DATA-length cross-check in
        // `route_data` (which is role-agnostic — inbound DATA on a client stream is the response
        // body). Set on the final response only; interim responses returned above carry no body.
        let expected_content_length = field_section.headers().content_length();

        let entry = self
            .streams
            .get_mut(&stream_id)
            .expect("caller verified stream is present");
        entry.expected_content_length = expected_content_length;
        let state = entry.shared.clone();

        // Latch the "we've seen the first HEADERS" flag *before* writing the slot. Subsequent
        // HEADERS arrivals route as trailers via this flag rather than the slot's occupancy,
        // since the conn task takes the slot when consuming headers.
        state
            .recv
            .first_response_headers_seen
            .store(true, Ordering::Release);

        // Stash headers, then transition recv via the lifecycle if `END_STREAM`. Order: headers
        // slot first, then lifecycle — by the time a recv reader observes recv-closed,
        // response_headers is also visible.
        *state
            .recv
            .response_headers
            .lock()
            .expect("response_headers mutex poisoned") = Some(Box::new(field_section));
        if end_stream {
            let _ = state.apply_event(StreamEvent::RecvHeaders { end_stream: true });
        }
        state.recv.response_headers_waker.wake();
        if end_stream {
            state.recv.waker.wake();
            self.close_if_both_done(stream_id);
        }
    }

    /// Server-role handler for HEADERS on a stream id we've not seen before: open the
    /// stream, validate the request via [`Conn::new_h2`], and emit the [`Conn`] on
    /// success. On rejection the stream is dropped with `RST_STREAM` before a handler task
    /// ever sees it.
    fn finalize_new_request_stream(
        &mut self,
        stream_id: u32,
        end_stream: bool,
        field_section: crate::headers::hpack::FieldSection<'static>,
    ) -> Action {
        let state = Arc::new(StreamState::default());
        // Opening HEADERS: Idle → Open (or HalfClosedRemote if it carried END_STREAM).
        let _ = state.apply_event(StreamEvent::RecvHeaders { end_stream });
        let send_window = i64::from(
            self.connection
                .current_peer_settings()
                .effective_initial_window_size(),
        );
        // Seed the recv window at what we advertised in SETTINGS_INITIAL_WINDOW_SIZE
        // (`HttpConfig::h2_initial_stream_window_size`). The lazy two-tier promotion to the larger
        // read-target happens in `service_handler_signals` once the handler signals `is_reading`.
        let stream_inflow = Inflow::new(i64::from(self.config.initial_stream_window_size()));
        // Declared body length, for the content-length / DATA-length cross-check in
        // `route_data`. A malformed value reads as "no declared length" here; the conn task's
        // `ValidatedRequest` rejects it independently, resetting the stream before any DATA flows.
        let expected_content_length = field_section.headers().content_length();
        // The request's `priority` header is the initial scheduling signal, until a
        // `PRIORITY_UPDATE` overrides it. A malformed value parses to the default rather than
        // erroring (the scheme mandates graceful degradation).
        let priority = field_section
            .headers()
            .get_str(KnownHeaderName::Priority)
            .map_or_default(Priority::parse);
        log::trace!("h2 stream {stream_id}: initial priority \"{priority}\"");
        self.connection
            .streams_lock()
            .insert(stream_id, state.clone());
        self.streams.insert(
            stream_id,
            StreamEntry::new(
                state.clone(),
                send_window,
                stream_inflow,
                expected_content_length,
                priority,
            ),
        );
        self.last_peer_stream_id = stream_id;

        let transport = H2Transport::new(self.connection.clone(), stream_id, state);
        let start_time = *self.burst_start.get_or_insert_with(Instant::now);
        match Conn::new_h2(
            self.connection.clone(),
            stream_id,
            field_section,
            transport,
            start_time,
        ) {
            Ok(conn) => Action::Emit(conn),
            Err(code) => {
                log::debug!("h2 stream {stream_id}: rejected during build: {code:?}");
                self.streams.remove(&stream_id);
                self.connection.streams_lock().remove(&stream_id);
                self.queue_rst_stream(stream_id, code);
                Action::Continue
            }
        }
    }

    /// Receive-side trailers: stash on `StreamState.recv.trailers` and signal EOF.
    /// Pseudo-header or missing-END_STREAM violations are stream-level errors —
    /// `RST_STREAM(PROTOCOL_ERROR)` and leave the connection alive.
    fn finalize_trailers(
        &mut self,
        stream_id: u32,
        end_stream: bool,
        field_section: crate::headers::hpack::FieldSection<'static>,
    ) {
        let (pseudos, trailers) = field_section.into_parts();
        if !end_stream || !pseudos.is_empty() {
            log::debug!(
                "h2 stream {stream_id}: malformed trailers (end_stream={end_stream}, \
                 pseudos_empty={})",
                pseudos.is_empty()
            );
            self.queue_rst_stream(stream_id, H2ErrorCode::ProtocolError);
            self.complete_and_remove_stream(stream_id, Err(Error::other("malformed h2 trailers")));
            return;
        }

        // Shared state lookup via our driver-private `StreamEntry` avoids re-locking the
        // shared map. Both point at the same Arc<StreamState>.
        let entry = self
            .streams
            .get(&stream_id)
            .expect("caller verified stream is present");
        let state = &entry.shared;

        // Race ordering: store trailers first, then transition recv via the lifecycle. The
        // lifecycle transition is the publication edge — observers that load recv-closed
        // through the lifecycle see the trailers populated first.
        *state
            .recv
            .trailers
            .lock()
            .expect("recv trailers mutex poisoned") = Some(Box::new(trailers));
        // Trailing HEADERS always carry END_STREAM (validated above).
        let _ = state.apply_event(StreamEvent::RecvHeaders { end_stream: true });
        state.recv.waker.wake();
        // Recv is now closed; if our send half already completed, tear the stream down (server) or
        // signal close keeping it for trailer access (client).
        self.close_if_both_done(stream_id);
    }
}
