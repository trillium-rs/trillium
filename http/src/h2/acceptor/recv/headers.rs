//! HEADERS + CONTINUATION block accumulation and finalization.
//!
//! `handle_headers` and `handle_continuation` are the per-frame entry points called from the
//! parent `recv::dispatch`. Once `END_HEADERS` is observed, [`Self::finalize_headers`]
//! HPACK-decodes the assembled block and branches by stream state + role:
//! - **New peer-initiated stream (server role)** → [`Self::finalize_new_request_stream`] opens the
//!   stream, validates the request, and emits a [`Conn`] for the handler task.
//! - **Existing stream, client role, no first response yet** → [`Self::finalize_response_headers`]
//!   stashes the response HEADERS for the conn task.
//! - **Existing stream, anything else** → [`Self::finalize_trailers`] stashes trailers and signals
//!   EOF.
//!
//! All methods are on [`super::super::H2Driver`].

use crate::{
    Conn, Status,
    h2::{
        H2Error, H2ErrorCode,
        acceptor::{Action, CloseOutcome, H2Driver, Role, StreamEntry, frame_slice},
        frame::FRAME_HEADER_LEN,
        transport::{H2Transport, StreamState},
    },
    headers::hpack::HpackDecodeError,
};
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::sync::{Arc, atomic::Ordering};

/// HEADERS + CONTINUATION assembly state.
#[derive(Debug)]
pub(in crate::h2::acceptor) struct PendingHeaders {
    pub(super) stream_id: u32,
    pub(super) end_stream: bool,
    pub(super) assembled: Vec<u8>,
}

impl<T> H2Driver<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    /// A HEADERS frame arrived. Either `END_HEADERS` is set (emit the stream immediately) or
    /// we accumulate the fragment into `pending_headers` and wait for CONTINUATION.
    ///
    /// A HEADERS frame on an *existing* stream is trailers. Accumulation is identical to an
    /// initial HEADERS block; the branch between "initial request HEADERS" and "trailers"
    /// happens in [`Self::finalize_headers`] against the current streams map.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn handle_headers(
        &mut self,
        stream_id: u32,
        end_stream: bool,
        end_headers: bool,
        priority: Option<crate::h2::frame::PriorityInfo>,
        header_block_length: u32,
        payload_start: usize,
        total: usize,
    ) -> Result<Action, CloseOutcome> {
        // A peer-initiated stream id must be odd.
        if stream_id.is_multiple_of(2) {
            return Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError));
        }
        // Trailer HEADERS on an existing stream: must be strictly equal to a known id.
        // New-stream HEADERS: strictly greater than `last_peer_stream_id`. A lower id
        // that is no longer active splits three ways:
        // - Closed via `RST_STREAM` (either direction) → stream-level `STREAM_CLOSED`. Ledger
        //   lookup returns `ClosedReason::Reset`.
        // - Closed via `END_STREAM` on both sides → connection-level `STREAM_CLOSED`. Ledger lookup
        //   returns `ClosedReason::EndStream`.
        // - Never opened (implicitly closed by a higher-id HEADERS, or evicted from the bounded
        //   ledger) → connection-level `PROTOCOL_ERROR` per the spec's "stream identifiers MUST be
        //   numerically greater than all streams the initiating endpoint has opened".
        let is_new_stream = !self.streams.contains_key(&stream_id);
        if is_new_stream && stream_id <= self.last_peer_stream_id {
            match self.closed_reason(stream_id) {
                Some(super::super::ClosedReason::Reset) => {
                    self.queue_rst_stream(stream_id, H2ErrorCode::StreamClosed);
                    return Ok(Action::Continue);
                }
                Some(super::super::ClosedReason::EndStream) => {
                    return Err(CloseOutcome::Protocol(H2ErrorCode::StreamClosed));
                }
                None => {
                    return Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError));
                }
            }
        }

        // A stream cannot depend on itself. Stream-level PROTOCOL_ERROR — the connection
        // stays alive; just RST this stream and advance past it.
        if let Some(p) = priority
            && p.stream_dependency == stream_id
        {
            log::debug!("h2 stream {stream_id}: HEADERS depends on itself; RST_STREAM");
            self.queue_rst_stream(stream_id, H2ErrorCode::ProtocolError);
            // Advance `last_peer_stream_id` so the peer can't reuse this id.
            if is_new_stream {
                self.last_peer_stream_id = stream_id;
            }
            return Ok(Action::Continue);
        }

        // Peer-initiated streams beyond our advertised `SETTINGS_MAX_CONCURRENT_STREAMS`
        // get `RST_STREAM(RefusedStream)`. The identifier is still "consumed" ("The
        // identifier of a refused stream is not reused") so bump `last_peer_stream_id`.
        let max_concurrent = self.config.max_concurrent_streams() as usize;
        if is_new_stream && self.streams.len() >= max_concurrent {
            log::debug!(
                "h2 stream {stream_id}: concurrent stream limit reached ({max_concurrent})"
            );
            self.queue_rst_stream(stream_id, H2ErrorCode::RefusedStream);
            self.last_peer_stream_id = stream_id;
            return Ok(Action::Continue);
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
    pub(super) fn handle_continuation(
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
    /// HEADERS + CONTINUATION*). Branches on whether the stream is already open:
    /// - **New stream:** HPACK-decode, open the stream, validate the request via [`Conn::new_h2`],
    ///   emit the [`Conn`] on success; on a malformed-request rejection, queue
    ///   `RST_STREAM(PROTOCOL_ERROR)` and drop the stream before a handler task ever sees it.
    /// - **Existing stream (trailers):** HPACK-decode, validate `END_STREAM` is set and no
    ///   pseudo-headers present, stash on `StreamState.recv.trailers`, then signal EOF. A
    ///   stream-level violation queues `RST_STREAM(PROTOCOL_ERROR)` on the offending stream and
    ///   leaves the connection open.
    ///
    /// HPACK decode failures split by variant: wire-format compression errors are
    /// connection-level (dynamic table now untrusted for every future stream — bubble up as
    /// `CloseOutcome::Protocol`), while spec-defined request malformation is stream-level
    /// (`RST_STREAM(PROTOCOL_ERROR)` and the connection continues).
    fn finalize_headers(
        &mut self,
        stream_id: u32,
        end_stream: bool,
        block: &[u8],
    ) -> Result<Action, CloseOutcome> {
        let field_section = match self.hpack.decode(block) {
            Ok(fs) => fs,
            Err(HpackDecodeError::Compression(e)) => {
                let h2err: H2Error = e.into();
                return Err(match h2err {
                    H2Error::Protocol(code) => CloseOutcome::Protocol(code),
                    H2Error::Io(e) => CloseOutcome::Io(e),
                });
            }
            Err(HpackDecodeError::MalformedRequest(reason)) => {
                log::debug!("h2 stream {stream_id}: malformed request headers: {reason:?}");
                // Stream-level. Pre-stream-open: just RST; post-open (trailer path):
                // drive through the completion helper to clean up send state if any.
                if self.streams.contains_key(&stream_id) {
                    self.queue_rst_stream(stream_id, H2ErrorCode::ProtocolError);
                    self.complete_and_remove_stream(
                        stream_id,
                        Err(std::io::Error::other(
                            "malformed h2 request trailer header block",
                        )),
                    );
                } else {
                    self.queue_rst_stream(stream_id, H2ErrorCode::ProtocolError);
                }
                return Ok(Action::Continue);
            }
        };

        if let Some(entry) = self.streams.get(&stream_id) {
            // Half-closed (remote): once we've observed the peer's `END_STREAM`, any
            // further HEADERS on that stream is a stream-level `STREAM_CLOSED`. This is
            // the case where `recv.eof` was already set by a prior DATA(END_STREAM) or a
            // prior trailer HEADERS — trailers themselves arrive while the stream is
            // still "open" and flip `eof` as part of the transition, so this check
            // correctly picks out "second HEADERS after eof flipped", not the trailer
            // itself. Role-agnostic: applies symmetrically to server (peer-sent EOS) and
            // client (peer-sent response EOS).
            if entry.shared.lifecycle_lock().recv_eof() {
                log::debug!("h2 stream {stream_id}: HEADERS on half-closed-remote stream");
                self.queue_rst_stream(stream_id, H2ErrorCode::StreamClosed);
                self.complete_and_remove_stream(
                    stream_id,
                    Err(std::io::Error::other(
                        "HEADERS on half-closed-remote h2 stream",
                    )),
                );
                return Ok(Action::Continue);
            }
            // Role-asymmetric: server treats HEADERS-on-known as trailers; client first
            // arrival is the response headers, subsequent arrival is trailers. We
            // distinguish by whether the response_headers slot has already been populated.
            match self.role {
                Role::Server => self.finalize_trailers(stream_id, end_stream, field_section),
                Role::Client => {
                    // Latching flag, not slot occupancy: the conn task drains
                    // `response_headers` when it consumes them, so the slot would falsely
                    // read empty for a trailing-HEADERS arriving after the conn task has
                    // taken the response.
                    let already_have_response = entry
                        .shared
                        .recv
                        .first_response_headers_seen
                        .load(Ordering::Acquire);
                    if already_have_response {
                        // Second HEADERS arrival on a client-initiated stream is the
                        // trailing HEADERS — same constraints as the server-side trailer
                        // path (no pseudos, MUST carry END_STREAM).
                        self.finalize_trailers(stream_id, end_stream, field_section);
                    } else {
                        self.finalize_response_headers(stream_id, end_stream, field_section);
                    }
                }
            }
            return Ok(Action::Continue);
        }

        // Role-asymmetric: server opens a new request stream; client would see this only
        // as a server-initiated push. We don't enable PUSH in our advertised SETTINGS, so
        // a peer-initiated even-id HEADERS would be a protocol violation; an odd-id HEADERS
        // we don't have means we already declared this id wasn't valid. Either way, refuse.
        Ok(match self.role {
            Role::Server => self.finalize_new_request_stream(stream_id, end_stream, field_section),
            Role::Client => {
                log::debug!(
                    "h2 client: HEADERS on unknown stream {stream_id} — refusing (push disabled)"
                );
                self.queue_rst_stream(stream_id, H2ErrorCode::RefusedStream);
                Action::Continue
            }
        })
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
                state.lifecycle_lock().mark_recv_eof();
                state.recv.response_headers_waker.wake();
                state.recv.waker.wake();
                self.try_close_if_both_done(stream_id);
            }
            return;
        }

        let entry = self
            .streams
            .get(&stream_id)
            .expect("caller verified stream is present");
        let state = entry.shared.clone();

        // Latch the "we've seen the first HEADERS" flag *before* writing the slot. Subsequent
        // HEADERS arrivals route as trailers via this flag rather than the slot's occupancy,
        // since the conn task takes the slot when consuming headers.
        state
            .recv
            .first_response_headers_seen
            .store(true, Ordering::Release);

        // Stash headers, then flip recv-eof via the lifecycle if appropriate. Order:
        // headers slot first, then lifecycle — by the time a recv reader observes eof,
        // response_headers is also visible.
        *state
            .recv
            .response_headers
            .lock()
            .expect("response_headers mutex poisoned") = Some(field_section);
        if end_stream {
            state.lifecycle_lock().mark_recv_eof();
        }
        state.recv.response_headers_waker.wake();
        if end_stream {
            state.recv.waker.wake();
            self.try_close_if_both_done(stream_id);
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
        if end_stream {
            state.lifecycle_lock().mark_recv_eof();
        }
        let send_window = i64::from(
            self.connection
                .current_peer_settings()
                .effective_initial_window_size(),
        );
        // Peer's recv window seed = what we advertised in SETTINGS_INITIAL_WINDOW_SIZE.
        // Default is 0 (lazy-WU pattern) — the peer cannot send body bytes until the
        // handler calls `H2Transport::poll_read` and the driver observes `is_reading` on
        // a subsequent tick. Configurable via `HttpConfig::h2_initial_stream_window_size`.
        let peer_recv_window = i64::from(self.config.initial_stream_window_size());
        self.connection
            .streams_lock()
            .insert(stream_id, state.clone());
        self.streams.insert(
            stream_id,
            StreamEntry::new(state.clone(), send_window, peer_recv_window),
        );
        self.last_peer_stream_id = stream_id;

        let transport = H2Transport::new(self.connection.clone(), stream_id, state);
        match Conn::new_h2(self.connection.clone(), stream_id, field_section, transport) {
            Ok(conn) => Action::Emit(Box::new(conn)),
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
            self.complete_and_remove_stream(
                stream_id,
                Err(std::io::Error::other("malformed h2 trailers")),
            );
            return;
        }

        // Shared state lookup via our driver-private `StreamEntry` avoids re-locking the
        // shared map. Both point at the same Arc<StreamState>.
        let entry = self
            .streams
            .get(&stream_id)
            .expect("caller verified stream is present");
        let state = &entry.shared;

        // Race ordering: store trailers first, then flip recv-eof via the lifecycle.
        // The lifecycle transition is the publication edge — observers that load eof
        // through the lifecycle see the trailers populated first.
        *state
            .recv
            .trailers
            .lock()
            .expect("recv trailers mutex poisoned") = Some(trailers);
        state.lifecycle_lock().mark_recv_eof();
        state.recv.waker.wake();
    }
}
