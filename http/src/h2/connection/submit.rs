//! Conn-task → driver submission API.
//!
//! Server-side: [`H2Connection::submit_send`][super::H2Connection::submit_send] /
//! [`submit_upgrade`][super::H2Connection::submit_upgrade] hand a response off to the driver for
//! framing. Client-side: [`open_stream`][super::H2Connection::open_stream] /
//! [`open_connect_stream`][super::H2Connection::open_connect_stream] allocate a fresh
//! peer-initiated stream id and stage a request.
//!
//! All four entry points share the same shape: stage a batch of [`OutboundPart`]s on the stream's
//! send queue, raise `needs_servicing`, wake the driver. A `Close` terminator is included for a
//! determinate send (normal response / request) and omitted for a bidirectional upgrade, which
//! stays open. The returned [`SubmitSend`] future resolves once the queue first drains (the
//! prelude handoff for an upgrade; `END_STREAM` otherwise).

use super::H2Connection;
#[cfg(feature = "unstable")]
use crate::h2::transport::H2Transport;
use crate::{
    Body, Headers,
    h2::transport::{OutboundPart, StreamState},
    headers::hpack::PseudoHeaders,
};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, atomic::Ordering},
    task::{Context, Poll},
};

/// Future returned by the various send-staging primitives on [`H2Connection`]; resolves once
/// the driver has fully framed and flushed the submitted message (request on the client,
/// response on the server), or with the relevant `io::Error` on failure.
#[must_use = "futures do nothing unless awaited"]
#[derive(Debug)]
pub struct SubmitSend {
    pub(super) stream_id: u32,
    /// `None` if the stream wasn't in the map at submit time (already closed). The future
    /// surfaces that as `NotConnected`.
    pub(super) stream: Option<Arc<StreamState>>,
}

impl Future for SubmitSend {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Some(state) = &self.stream else {
            log::debug!("h2 stream {}: submit_send on closed stream", self.stream_id);
            return Poll::Ready(Err(io::ErrorKind::NotConnected.into()));
        };

        let stream_id = self.stream_id;
        let try_take = || -> Option<io::Result<()>> {
            state.send.submit_resolved.load(Ordering::Acquire).then(|| {
                state
                    .send
                    .completion_result
                    .lock()
                    .expect("completion_result mutex poisoned")
                    .take()
                    .unwrap_or_else(|| {
                        log::error!(
                            "h2 stream {stream_id}: submit_resolved without a completion_result — \
                             driver should write the result before flipping submit_resolved"
                        );
                        Ok(())
                    })
            })
        };

        if let Some(result) = try_take() {
            return Poll::Ready(result);
        }
        // A connection that dies mid-flight (i/o error, peer FIN, GOAWAY) moves every live
        // stream to `Closed{Reset}` via the driver's teardown without ever flipping
        // `submit_resolved` — the driver exited before framing this submission. A handler woken
        // by that teardown then submits into a dead connection; resolve with an error so it
        // doesn't park forever holding its swansong guard. `is_reset` (not `is_closed`) so a
        // clean send isn't misreported in the window before its `submit_resolved` is visible.
        if state.lifecycle_lock().is_reset() {
            return Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()));
        }
        state.send.completion_waker.register(cx.waker());
        // Re-check after registering so we don't miss a wake fired between the loads above and
        // the registration (both the normal completion and the connection-death reset).
        if let Some(result) = try_take() {
            return Poll::Ready(result);
        }
        if state.lifecycle_lock().is_reset() {
            return Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()));
        }
        Poll::Pending
    }
}

impl H2Connection {
    /// Hand a response off to the driver for framing and transmission.
    ///
    /// The conn task stages owned `pseudos + headers + body` in the per-stream submission
    /// slot and `await`s the returned future. The driver HPACK-encodes the headers, frames
    /// HEADERS + DATA, and signals completion.
    ///
    /// Trailers are not a separate argument: the driver pulls them off the body via
    /// [`Body::trailers`] once the body is fully drained.
    pub(crate) fn submit_send(
        &self,
        stream_id: u32,
        pseudos: PseudoHeaders<'static>,
        headers: Headers,
        body: Option<Body>,
    ) -> SubmitSend {
        let stream = self.streams_lock().get(&stream_id).cloned();
        if let Some(state) = &stream {
            state.stage(submission_parts(pseudos, headers, body, true));
            self.outbound_waker.wake();
        }
        SubmitSend { stream_id, stream }
    }

    /// Hand a response off for an extended-CONNECT (RFC 8441) upgrade.
    ///
    /// Frames the response HEADERS without `END_STREAM`, then the optional `body` (a prelude
    /// sent before the upgrade transition) as DATA. [`SubmitSend`] completion is signaled
    /// once the prelude is fully framed — so the conn task returns and the upgrade handler
    /// runs only after the prelude is on the wire (matching h1/h3), not at `END_HEADERS`.
    ///
    /// After the prelude drains, the send pump switches to sourcing DATA from the per-stream
    /// outbound queue: the upgrade handler writes bytes through the returned transport and
    /// the pump frames them, bounded by per-stream + connection send windows. Closing or
    /// dropping the transport emits the terminator and tears the stream down.
    pub(crate) fn submit_upgrade(
        &self,
        stream_id: u32,
        pseudos: PseudoHeaders<'static>,
        headers: Headers,
        body: Option<Body>,
    ) -> SubmitSend {
        let stream = self.streams_lock().get(&stream_id).cloned();
        if let Some(state) = &stream {
            state.stage(submission_parts(pseudos, headers, body, false));
            log::trace!("h2 stream {stream_id}: submit_upgrade — parts staged");
            self.outbound_waker.wake();
        }
        SubmitSend { stream_id, stream }
    }

    /// Stage trailing HEADERS for an active upgrade stream and close the outbound write
    /// half. Fire-and-forget — the driver task emits the trailing HEADERS frame with
    /// `END_STREAM` and tears the stream down.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::NotConnected`] if the stream is no longer in the
    /// connection's streams map.
    pub(crate) fn submit_trailers(&self, stream_id: u32, trailers: Headers) -> io::Result<()> {
        let stream = self
            .streams_lock()
            .get(&stream_id)
            .cloned()
            .ok_or(io::ErrorKind::NotConnected)?;
        // Enqueue trailing HEADERS as the terminator (it carries `END_STREAM`). The send pump
        // drains the streaming ring first, then frames these. Best-effort: if the send half is
        // already closed the part is harmlessly ignored, so we don't gate on state here.
        stream.stage([OutboundPart::Trailers(trailers)]);
        stream.send.outbound_write_waker.wake();
        self.outbound_waker.wake();
        log::trace!("h2 stream {stream_id}: submit_trailers staged trailing HEADERS terminator");
        Ok(())
    }

    /// Client-role primitive: allocate a fresh outbound stream id, stage a request submission
    /// for the driver, and return the id, a [`SubmitSend`] tracking the request's send half,
    /// and the per-stream [`H2Transport`] for response-body reads.
    ///
    /// `pseudos + headers` are handed owned to the driver, which encodes them synchronously
    /// at submission pickup. `body` is the request body, if any; `None` causes the HEADERS
    /// frame to carry `END_STREAM` and no DATA to be emitted.
    ///
    /// Returns `None` when:
    /// - The 2^31 odd-id space is exhausted (caller should fail over to a new connection), or
    /// - The connection is shutting down (GOAWAY received or local shutdown requested).
    ///
    /// The returned [`SubmitSend`] resolves once the request has been fully framed and
    /// flushed, or with the relevant `io::Error` on failure. The response side is awaited
    /// separately via [`response_headers`][Self::response_headers] for the response HEADERS,
    /// and the [`H2Transport`]'s `AsyncRead` impl for the response body.
    ///
    /// **`SubmitSend` is drop-safe.** Once handed off here, the body is owned by the driver,
    /// which continues to drain it, frame DATA, emit trailers / `END_STREAM`, and tear the
    /// stream down whether or not the caller awaits the returned future. Clients that only
    /// care about the response (the typical case) may drop it without polling.
    ///
    /// # Panics
    ///
    /// Panics if any per-connection or per-stream mutex is poisoned.
    #[cfg(feature = "unstable")]
    pub fn open_stream(
        self: &Arc<Self>,
        pseudos: PseudoHeaders<'static>,
        headers: Headers,
        body: Option<Body>,
    ) -> Option<(u32, SubmitSend, H2Transport)> {
        self.open_stream_inner(pseudos, headers, body, false)
            .map(|(id, state, transport)| {
                (
                    id,
                    SubmitSend {
                        stream_id: id,
                        stream: Some(state),
                    },
                    transport,
                )
            })
    }

    /// Client-role: open a stream for an extended-CONNECT bootstrap (RFC 8441 for
    /// WebSocket-over-h2; `draft-ietf-webtrans-http2` for WebTransport-over-h2).
    ///
    /// `headers_plan` is the HPACK plan for the HEADERS block; the caller is responsible
    /// for ensuring it carries `:method = CONNECT` and a `:protocol` pseudo-header. The
    /// initial HEADERS is sent without `END_STREAM`; the optional `body` (a prelude sent
    /// before the upgrade transition) goes out as DATA, then the per-stream outbound queue
    /// stays open until the application closes the returned transport.
    ///
    /// Returns `(stream_id, SubmitSend, H2Transport)`. The caller `await`s the [`SubmitSend`]
    /// so control returns only once the prelude is on the wire (matching h1/h3), then reads
    /// response HEADERS via [`Self::response_headers`] and exchanges bytes over the returned
    /// transport's `AsyncRead` + `AsyncWrite`.
    ///
    /// Returns `None` under the same conditions as [`Self::open_stream`]: stream-id space
    /// exhausted, or connection shutting down.
    ///
    /// **Caller MUST first await [`peer_settings`][Self::peer_settings] and verify the
    /// returned snapshot's `enable_connect_protocol() == Some(true)` before calling this.**
    /// Sending extended CONNECT to a peer that hasn't advertised
    /// `SETTINGS_ENABLE_CONNECT_PROTOCOL = 1` is a protocol violation.
    #[cfg(feature = "unstable")]
    pub fn open_connect_stream(
        self: &Arc<Self>,
        pseudos: PseudoHeaders<'static>,
        headers: Headers,
        body: Option<Body>,
    ) -> Option<(u32, SubmitSend, H2Transport)> {
        self.open_stream_inner(pseudos, headers, body, true)
            .map(|(id, state, transport)| {
                (
                    id,
                    SubmitSend {
                        stream_id: id,
                        stream: Some(state),
                    },
                    transport,
                )
            })
    }

    /// Shared id-allocate-and-stage logic backing [`Self::open_stream`] and
    /// [`Self::open_connect_stream`]. The `is_upgrade` flag means HEADERS does not carry
    /// `END_STREAM` and the stream stays open after the body drains — the send pump then
    /// sources the post-handoff continuation from the per-stream outbound ring (the bytes
    /// the handler writes through `H2Transport`). The caller-supplied `body` is the prelude
    /// in both cases; for the non-upgrade path `END_STREAM` semantics fall out of
    /// `body.is_none()`.
    #[cfg(feature = "unstable")]
    fn open_stream_inner(
        self: &Arc<Self>,
        pseudos: PseudoHeaders<'static>,
        headers: Headers,
        body: Option<Body>,
        is_upgrade: bool,
    ) -> Option<(u32, Arc<StreamState>, H2Transport)> {
        if !self.swansong.state().is_running() {
            return None;
        }

        let stream_id = self
            .next_client_stream_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < (1u32 << 31)).then_some(n + 2)
            })
            .ok()?;

        let state = Arc::new(StreamState::default());

        // Stage the request parts *before* publishing the stream id to the shared map so they're
        // visible the first time the driver sees the stream — otherwise a second tick would be
        // needed to start framing. A non-upgrade request closes after its body; an extended-CONNECT
        // bootstrap stays open.
        state.stage(submission_parts(pseudos, headers, body, !is_upgrade));
        self.streams_lock().insert(stream_id, state.clone());
        log::trace!("h2 client: open_stream allocated stream {stream_id} (upgrade={is_upgrade})");
        self.outbound_waker.wake();
        let transport = H2Transport::new(Arc::clone(self), stream_id, state.clone());
        Some((stream_id, state, transport))
    }
}

/// Build the ordered [`OutboundPart`]s for a submission: the HEADERS block, an optional body, and
/// — for a determinate send — a `Close` terminator. An extended-CONNECT upgrade passes
/// `close = false` so the stream stays open for the bidirectional phase.
fn submission_parts(
    pseudos: PseudoHeaders<'static>,
    headers: Headers,
    body: Option<Body>,
    close: bool,
) -> Vec<OutboundPart> {
    let mut parts = Vec::with_capacity(3);
    parts.push(OutboundPart::Headers { pseudos, headers });
    if let Some(body) = body {
        parts.push(OutboundPart::Body(body));
    }
    if close {
        parts.push(OutboundPart::Close);
    }
    parts
}
