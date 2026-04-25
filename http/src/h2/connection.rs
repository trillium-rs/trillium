//! Shared per-connection HTTP/2 state ([`H2Connection`]) plus the [`SubmitSend`] future
//! conn tasks await for response transmission.
//!
//! [`H2Connection`] is `Arc`-shared between the driver task ([`H2Driver`]) and every conn
//! task that holds an open stream's [`Conn`]. It owns the per-stream `StreamState` map,
//! the cross-task wake primitive ([`AtomicWaker`]), and the [`HttpContext`] / [`Swansong`]
//! the broader server stack reaches in through.
//!
//! The driver loop itself lives in [`super::acceptor`] — see that module for the
//! per-connection state machine and how send / receive concerns are split.
//!
//! [`H2Driver`]: super::H2Driver

use super::{H2Driver, H2Settings, acceptor::Role, transport::StreamState};
use crate::{Body, Conn, Headers, HttpContext};
use atomic_waker::AtomicWaker;
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    collections::HashMap,
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard, atomic::Ordering},
    task::{Context, Poll},
};
use swansong::{ShutdownCompletion, Swansong};

/// Shared per-connection state for HTTP/2.
///
/// Wrapped in an [`Arc`] and held by both the [`H2Driver`] driver and every conn task
/// that holds an open stream's [`Conn`]. Per-stream `StreamState`, HPACK encoder state, and
/// connection-level send flow control will accumulate here as later phases land.
#[derive(Debug)]
pub struct H2Connection {
    context: Arc<HttpContext>,
    swansong: Swansong,
    /// Driver-side waker that conn tasks fire whenever they produce work the driver should
    /// act on — the is-reading signal on first `H2Transport::poll_read`, and the
    /// `submit_send` arrival. Single-consumer (the driver); N producers (conn tasks). The
    /// driver registers its current `drive` waker here each iteration it parks.
    outbound_waker: AtomicWaker,
    /// Per-stream shared state, keyed by stream id. The driver inserts on stream open and
    /// removes on close. Conn-task-side code (`ReceivedBody`, `Conn::send_h2`) looks up
    /// via private accessor methods on `H2Connection` rather than touching the map
    /// directly — `StreamState` stays module-private. The driver also caches each
    /// `Arc<StreamState>` in its private `StreamEntry` for hot-loop perf, so every entry
    /// here has refcount ≥ 2 while the stream is open.
    streams: Mutex<HashMap<u32, Arc<StreamState>>>,
    /// The peer's most recently announced SETTINGS values. Written by the driver each time a
    /// SETTINGS frame arrives (or, for the initial SETTINGS, the first one); read from the
    /// driver's send path when it needs to respect peer-advertised limits (HEADERS fragment
    /// size, stream send-window seed, `MAX_HEADER_LIST_SIZE` cap). Single-task access (only
    /// the driver touches this), so a plain `Mutex` suffices — the `RwLock` optimisation for
    /// concurrent shared reads would be wasted here. `H2Settings` is `Copy`, so readers
    /// typically take the guard, copy out, and release.
    ///
    /// Default-constructed (all fields `None`) means "peer has not yet sent SETTINGS";
    /// readers should use [`H2Settings::effective_*`][H2Settings::effective_max_frame_size]
    /// helpers that apply the RFC 9113 §6.5.2 defaults to absent fields.
    peer_settings: Mutex<H2Settings>,
}

impl H2Connection {
    /// Construct a new `H2Connection` to manage HTTP/2 for a single peer.
    pub fn new(context: Arc<HttpContext>) -> Arc<Self> {
        let swansong = context.swansong().child();
        Arc::new(Self {
            context,
            swansong,
            outbound_waker: AtomicWaker::new(),
            streams: Mutex::new(HashMap::new()),
            peer_settings: Mutex::new(H2Settings::default()),
        })
    }

    /// The [`HttpContext`] this connection was constructed with.
    pub fn context(&self) -> Arc<HttpContext> {
        self.context.clone()
    }

    /// The connection-scoped [`Swansong`]. Shuts down on peer GOAWAY or when the server-
    /// level swansong shuts down.
    pub fn swansong(&self) -> &Swansong {
        &self.swansong
    }

    /// Attempt graceful shutdown of this HTTP/2 connection.
    pub fn shut_down(&self) -> ShutdownCompletion {
        self.swansong.shut_down()
    }

    /// Driver-side wake primitive. Conn-task code calls
    /// `connection.outbound_waker().wake()` after producing work the driver should service
    /// (an `is_reading` signal, a `submit_send` slot fill).
    pub(super) fn outbound_waker(&self) -> &AtomicWaker {
        &self.outbound_waker
    }

    /// Lock the per-stream `StreamState` map. Used by the driver (insert at stream open,
    /// remove at close) and by conn-task lookups (e.g. `submit_send`).
    pub(super) fn streams_lock(&self) -> MutexGuard<'_, HashMap<u32, Arc<StreamState>>> {
        self.streams
            .lock()
            .expect("connection streams mutex poisoned")
    }

    /// Lock the peer's SETTINGS. Cheap; held only as long as the returned guard lives.
    /// Use the `effective_*` helpers on [`H2Settings`] to get a value with RFC defaults
    /// applied for fields the peer hasn't set; typical callers copy out via `*guard` and
    /// release immediately.
    pub(super) fn peer_settings(&self) -> MutexGuard<'_, H2Settings> {
        self.peer_settings
            .lock()
            .expect("peer_settings mutex poisoned")
    }

    /// Remove and return trailers stashed on the stream's recv state. Called by
    /// [`ReceivedBody`][crate::ReceivedBody]'s End transition after the request body is
    /// fully drained. Returns `None` if the stream is gone (already closed) or no trailers
    /// were received.
    pub(crate) fn take_trailers(&self, stream_id: u32) -> Option<Headers> {
        let stream = self.streams_lock().get(&stream_id).cloned()?;
        stream
            .recv
            .trailers
            .lock()
            .expect("recv trailers mutex poisoned")
            .take()
    }

    /// Request that the driver emit `RST_STREAM` on this stream with the given error code
    /// and clean up. Called from the conn-task side when something in its path (e.g. a
    /// body-read that detected a content-length violation — RFC 9113 §8.1.2.6) needs the
    /// stream torn down but can't touch the driver's private state directly.
    ///
    /// Side effects: stashes the code on `StreamState.pending_reset` and wakes the driver.
    /// A no-op if the stream is already gone from the shared map — that happens when the
    /// driver has already closed the stream for its own reasons. Idempotent; only the first
    /// call takes effect, subsequent calls see the slot still filled and do nothing.
    pub(crate) fn stream_error(&self, stream_id: u32, code: super::H2ErrorCode) {
        let Some(stream) = self.streams_lock().get(&stream_id).cloned() else {
            return;
        };
        let mut slot = stream
            .pending_reset
            .lock()
            .expect("pending_reset mutex poisoned");
        if slot.is_none() {
            *slot = Some(code);
            drop(slot);
            self.outbound_waker.wake();
        }
    }

    /// Bind this `H2Connection` to a TCP transport and return an [`H2Driver`] that drives
    /// the connection.
    ///
    /// The driver must be polled to completion via repeated calls to
    /// [`H2Driver::next`] (or its [`Stream`][futures_lite::stream::Stream] impl); each returned
    /// [`Conn`] should be spawned on its own task.
    pub fn run<T>(self: Arc<Self>, transport: T) -> H2Driver<T>
    where
        T: AsyncRead + AsyncWrite + Unpin + Send,
    {
        H2Driver::new(self, transport, Role::Server)
    }

    /// Per-stream entry point — call from the runtime adapter's spawned task for each
    /// [`Conn`] returned by [`H2Driver::next`]. Runs `handler` to produce the response,
    /// then `send_h2` to hand the framed response to the driver.
    ///
    /// Mirrors [`H3Connection::process_inbound_bidi`][crate::h3::H3Connection::process_inbound_bidi]'s
    /// role for h3, except the Conn is already built (the acceptor decoded HEADERS and
    /// validated the request before emitting), so this just runs the handler chain and
    /// sends.
    ///
    /// # Errors
    ///
    /// Returns the [`io::Error`] from `send_h2` if the body's `poll_read` errors or the
    /// underlying transport fails partway through the response.
    pub async fn process_inbound<Transport, Handler, Fut>(
        conn: Conn<Transport>,
        handler: Handler,
    ) -> io::Result<Conn<Transport>>
    where
        Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
        Handler: FnOnce(Conn<Transport>) -> Fut,
        Fut: Future<Output = Conn<Transport>>,
    {
        handler(conn).await.send_h2().await
    }

    /// Hand a fully-encoded response off to the driver for framing and transmission.
    ///
    /// The conn task pre-encodes the response HEADERS into `encoded_headers` (via the
    /// static-or-literal HPACK encoder — no shared state required), takes the response
    /// body off the [`Conn`], and `await`s the returned future. The driver picks up the
    /// submission on its next `service_handler_signals` tick, frames it, and signals
    /// completion.
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
                .expect("send submission mutex poisoned") = Some(super::transport::Submission {
                encoded_headers,
                body,
                is_upgrade: false,
            });
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
    /// Internally constructs an [`H2OutboundReader`][super::transport::H2OutboundReader]
    /// over the per-stream outbound queue ([`SendState::outbound`][outbound]) and submits
    /// it as the response body. The upgrade handler appends bytes via
    /// [`H2Transport`][super::H2Transport]'s `AsyncWrite::poll_write`; the driver's send
    /// pump pulls them via the body's `AsyncRead::poll_read` and frames them as DATA
    /// frames bounded by per-stream + connection send windows. When the handler closes
    /// the transport (or drops it), the reader returns `Ready(0)`, the send pump emits
    /// `DATA(END_STREAM)`, and the stream tears down via the normal
    /// `complete_and_remove_stream` path.
    ///
    /// [outbound]: super::transport::SendState::outbound
    pub(crate) fn submit_upgrade(&self, stream_id: u32, encoded_headers: Vec<u8>) -> SubmitSend {
        let stream = self.streams_lock().get(&stream_id).cloned();
        if let Some(state) = &stream {
            let reader = super::transport::H2OutboundReader::new(state.clone(), stream_id);
            let body = Body::new_streaming(reader, None);
            *state
                .send
                .submission
                .lock()
                .expect("send submission mutex poisoned") = Some(super::transport::Submission {
                encoded_headers,
                body: Some(body),
                is_upgrade: true,
            });
            log::trace!("h2 stream {stream_id}: submit_upgrade — submission staged");
            self.outbound_waker.wake();
        }
        SubmitSend { stream_id, stream }
    }
}

/// Future returned by [`H2Connection::submit_send`]; resolves once the driver has fully
/// framed and flushed the submitted response, or with the relevant `io::Error` on failure.
///
/// Holds the per-stream [`StreamState`] Arc (cloned out of the streams map at submit time),
/// not a connection backref + id — so dropping the future doesn't require another map
/// lookup and the conn task's wake registration stays local to the per-stream sync
/// primitives.
#[must_use = "futures do nothing unless awaited"]
#[derive(Debug)]
pub(crate) struct SubmitSend {
    stream_id: u32,
    /// `None` if the stream wasn't in the map at submit time (already closed). The future
    /// surfaces that as `NotConnected`.
    stream: Option<Arc<StreamState>>,
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
