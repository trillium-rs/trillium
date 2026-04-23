//! Conn-task → driver submission API.
//!
//! Server-side: [`H2Connection::submit_send`][super::H2Connection::submit_send] /
//! [`submit_upgrade`][super::H2Connection::submit_upgrade] hand a fully-encoded response off
//! to the driver for framing. Client-side: [`open_stream`][super::H2Connection::open_stream] /
//! [`open_connect_stream`][super::H2Connection::open_connect_stream] allocate a fresh
//! peer-initiated stream id and stage a request submission.
//!
//! All four entry points share the same shape: lock the streams map, populate the per-stream
//! [`SendState::submission`][crate::h2::transport::SendState] slot, raise
//! `needs_servicing`, wake the driver. The returned [`SubmitSend`] future resolves once the
//! driver signals send completion.

use super::H2Connection;
#[cfg(feature = "unstable")]
use crate::h2::transport::H2Transport;
use crate::{Body, h2::transport::StreamState};
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
///
/// Holds the per-stream [`StreamState`] Arc (cloned out of the streams map at submit time),
/// not a connection backref + id — so dropping the future doesn't require another map
/// lookup and the conn task's wake registration stays local to the per-stream sync
/// primitives.
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
            state.send.completed.load(Ordering::Acquire).then(|| {
                state
                    .send
                    .completion_result
                    .lock()
                    .expect("completion_result mutex poisoned")
                    .take()
                    .unwrap_or_else(|| {
                        log::error!(
                            "h2 stream {stream_id}: completed without a completion_result — \
                             driver should write the result before flipping completed"
                        );
                        Ok(())
                    })
            })
        };

        if let Some(result) = try_take() {
            return Poll::Ready(result);
        }
        state.send.completion_waker.register(cx.waker());
        // Re-check after registering so we don't miss a wake fired between the load above
        // and the registration.
        if let Some(result) = try_take() {
            return Poll::Ready(result);
        }
        Poll::Pending
    }
}

impl H2Connection {
    /// Hand a fully-encoded response off to the driver for framing and transmission.
    ///
    /// The conn task pre-encodes the response HEADERS into `encoded_headers` (via the
    /// static-or-literal HPACK encoder — no shared state required), takes the response
    /// body off the [`Conn`][crate::Conn], and `await`s the returned future. The driver
    /// picks up the submission on its next `service_handler_signals` tick, frames it, and
    /// signals completion.
    ///
    /// Trailers are not a separate argument: the driver pulls them off the body via
    /// [`Body::trailers`] once the body is fully drained, mirroring how h1's send path
    /// works.
    pub(crate) fn submit_send(
        &self,
        stream_id: u32,
        encoded_headers: Vec<u8>,
        body: Option<Body>,
    ) -> SubmitSend {
        let stream = self.streams_lock().get(&stream_id).cloned();
        if let Some(state) = &stream {
            *state
                .send
                .submission
                .lock()
                .expect("send submission mutex poisoned") =
                Some(crate::h2::transport::Submission {
                    encoded_headers,
                    body,
                    is_upgrade: false,
                });
            state.needs_servicing.store(true, Ordering::Release);
            self.outbound_waker.wake();
        }
        SubmitSend { stream_id, stream }
    }

    /// Hand a response off for an extended-CONNECT (RFC 8441) upgrade.
    ///
    /// Frames the response HEADERS without `END_STREAM` and signals
    /// [`SubmitSend`] completion the moment the HEADERS frame is on the wire — instead of
    /// after the body finishes, as [`submit_send`][Self::submit_send] does. That early
    /// completion lets [`Conn::send_h2`][crate::Conn::send_h2] return so the runtime
    /// adapter can dispatch [`Handler::upgrade`][trillium::Handler::upgrade] while the
    /// stream stays open as a bidirectional byte channel.
    ///
    /// Internally constructs an [`H2OutboundReader`][crate::h2::transport::H2OutboundReader]
    /// over the per-stream outbound queue ([`SendState::outbound`][outbound]) and submits
    /// it as the response body. The upgrade handler appends bytes via
    /// [`H2Transport`][crate::h2::transport::H2Transport]'s `AsyncWrite::poll_write`; the
    /// driver's send pump pulls them via the body's `AsyncRead::poll_read` and frames them
    /// as DATA frames bounded by per-stream + connection send windows. When the handler
    /// closes the transport (or drops it), the reader returns `Ready(0)`, the send pump
    /// emits `DATA(END_STREAM)`, and the stream tears down via the normal
    /// `complete_and_remove_stream` path.
    ///
    /// [outbound]: crate::h2::transport::SendState::outbound
    pub(crate) fn submit_upgrade(&self, stream_id: u32, encoded_headers: Vec<u8>) -> SubmitSend {
        let stream = self.streams_lock().get(&stream_id).cloned();
        if let Some(state) = &stream {
            let reader = crate::h2::transport::H2OutboundReader::new(state.clone(), stream_id);
            let body = Body::new_streaming(reader, None);
            *state
                .send
                .submission
                .lock()
                .expect("send submission mutex poisoned") =
                Some(crate::h2::transport::Submission {
                    encoded_headers,
                    body: Some(body),
                    is_upgrade: true,
                });
            log::trace!("h2 stream {stream_id}: submit_upgrade — submission staged");
            state.needs_servicing.store(true, Ordering::Release);
            self.outbound_waker.wake();
        }
        SubmitSend { stream_id, stream }
    }

    /// Client-role primitive: allocate a fresh outbound stream id, stage a request submission
    /// for the driver, and return the id, a [`SubmitSend`] tracking the request's send half,
    /// and the per-stream [`H2Transport`] for response-body reads.
    ///
    /// `encoded_headers` is the HPACK-encoded HEADERS block (static-or-literal — no shared
    /// dynamic-table state). `body` is the request body, if any; `None` causes the HEADERS
    /// frame to carry `END_STREAM` and no DATA to be emitted.
    ///
    /// Returns `None` when:
    /// - The 2^31 odd-id space is exhausted (caller should fail over to a new connection), or
    /// - The connection is shutting down (we've received GOAWAY or our own swansong has been asked
    ///   to shut down) — opening another stream would just produce a stream the peer has promised
    ///   to ignore.
    ///
    /// Staging is synchronous and infallible past the `None` checks: the submission is
    /// published via the per-stream [`SendState::submission`][submission] slot and the driver
    /// is woken via [`outbound_waker`][outbound_waker]. The driver's pickup pass observes the
    /// new id in the shared streams map, allocates per-stream flow-control state, and the
    /// existing send pump frames HEADERS + DATA + optional trailing HEADERS as if the
    /// submission had come from the server-side path.
    ///
    /// The returned [`SubmitSend`] resolves once the request has been fully framed and
    /// flushed, or with the relevant `io::Error` on failure. The response side is awaited
    /// separately via [`response_headers`][Self::response_headers] for the response HEADERS,
    /// and the [`H2Transport`]'s `AsyncRead` impl for the response body.
    ///
    /// **`SubmitSend` is drop-safe.** The body, once handed off here, is owned by the
    /// driver's per-stream `SendState`; the driver continues to drain it, frame DATA, emit
    /// trailers / `END_STREAM`, and tear the stream down regardless of whether the caller
    /// awaits or drops the returned `SubmitSend`. Clients that only care about the response
    /// (the typical case) may drop it without polling.
    ///
    /// # Panics
    ///
    /// Panics if any of the per-connection / per-stream mutexes is poisoned (a previous
    /// thread panicked while holding the lock) — same posture as the rest of the h2
    /// driver's mutex usage.
    ///
    /// [submission]: crate::h2::transport::SendState::submission
    /// [outbound_waker]: Self::outbound_waker
    #[cfg(feature = "unstable")]
    pub fn open_stream(
        self: &Arc<Self>,
        encoded_headers: Vec<u8>,
        body: Option<Body>,
    ) -> Option<(u32, SubmitSend, H2Transport)> {
        self.open_stream_inner(encoded_headers, body, false)
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

    /// Client-role: open a stream for an extended-CONNECT bootstrap (RFC 8441 §3 — WebSocket-
    /// over-h2; the in-progress `draft-ietf-webtrans-http2` for WebTransport-over-h2).
    ///
    /// `encoded_headers` is the HPACK-encoded HEADERS block; the caller is responsible for
    /// ensuring it carries `:method = CONNECT` and a `:protocol` pseudo-header. This is the
    /// only case where staging a stream without a request body is *not* terminated by
    /// `END_STREAM` on the initial HEADERS — instead, the per-stream outbound queue (the same
    /// one [`H2Transport`]'s `AsyncWrite::poll_write` appends to) becomes the request body
    /// and stays open until the application closes the transport.
    ///
    /// Returns `(stream_id, H2Transport)` — no [`SubmitSend`]. The application reads response
    /// HEADERS via [`Self::response_headers`] and then exchanges bytes over the returned
    /// transport's `AsyncRead` + `AsyncWrite`.
    ///
    /// Returns `None` under the same conditions as [`Self::open_stream`]: stream-id space
    /// exhausted, or connection shutting down.
    ///
    /// **Caller MUST first await
    /// [`peer_settings`][Self::peer_settings] and verify the
    /// returned snapshot's `enable_connect_protocol() == Some(true)` before calling this.**
    /// Sending extended CONNECT to a peer that hasn't advertised
    /// `SETTINGS_ENABLE_CONNECT_PROTOCOL = 1` is a protocol violation per RFC 8441 §3.
    #[cfg(feature = "unstable")]
    pub fn open_connect_stream(
        self: &Arc<Self>,
        encoded_headers: Vec<u8>,
    ) -> Option<(u32, H2Transport)> {
        let (id, _state, transport) = self.open_stream_inner(encoded_headers, None, true)?;
        Some((id, transport))
    }

    /// Shared id-allocate-and-stage logic backing [`Self::open_stream`] and
    /// [`Self::open_connect_stream`]. The `is_upgrade` flag controls two things in the driver's
    /// send pump: HEADERS does not carry `END_STREAM` (because the body field is `Some`), and
    /// the body is sourced from the per-stream outbound queue ([`H2OutboundReader`]) rather
    /// than the caller-supplied `Body`. For the non-upgrade path, the caller-supplied `body`
    /// is used as-is and `END_STREAM` semantics fall out of `body.is_none()`.
    ///
    /// [`H2OutboundReader`]: crate::h2::transport::H2OutboundReader
    #[cfg(feature = "unstable")]
    fn open_stream_inner(
        self: &Arc<Self>,
        encoded_headers: Vec<u8>,
        body: Option<Body>,
        is_upgrade: bool,
    ) -> Option<(u32, Arc<StreamState>, H2Transport)> {
        if !self.swansong.state().is_running() {
            return None;
        }

        let stream_id = {
            let mut next = self
                .next_client_stream_id
                .lock()
                .expect("next_client_stream_id mutex poisoned");
            if *next >= (1u32 << 31) {
                return None;
            }
            let id = *next;
            *next += 2;
            id
        };

        let state = Arc::new(StreamState::default());

        // For an extended-CONNECT bootstrap, the body field of the submission must be the
        // per-stream outbound queue — same shape the server-side `submit_upgrade` uses.
        // That keeps HEADERS flowing without END_STREAM and turns the per-stream
        // outbound buffer into the writeback channel reachable through `H2Transport`'s
        // `AsyncWrite`.
        let body = if is_upgrade {
            let reader = crate::h2::transport::H2OutboundReader::new(state.clone(), stream_id);
            Some(Body::new_streaming(reader, None))
        } else {
            body
        };

        // Stage submission *before* publishing the stream id to the shared map. The driver's
        // client-pickup pass scans the shared map, allocates a `StreamEntry`, and on the same
        // tick the existing submission-pickup loop promotes this submission to a `SendCursor`.
        // Doing it in this order means the submission is guaranteed visible the first time
        // the driver sees the stream — no second tick needed to start framing.
        *state
            .send
            .submission
            .lock()
            .expect("send submission mutex poisoned") = Some(crate::h2::transport::Submission {
            encoded_headers,
            body,
            is_upgrade,
        });
        state.needs_servicing.store(true, Ordering::Release);
        self.streams_lock().insert(stream_id, state.clone());
        log::trace!("h2 client: open_stream allocated stream {stream_id} (upgrade={is_upgrade})");
        self.outbound_waker.wake();
        let transport = H2Transport::new(Arc::clone(self), stream_id, state.clone());
        Some((stream_id, state, transport))
    }
}
