//! Shared per-connection HTTP/2 state ([`H2Connection`]) plus the [`SubmitSend`] future
//! conn tasks await for response transmission.
//!
//! [`H2Connection`] is `Arc`-shared between the driver task ([`H2Acceptor`]) and every conn
//! task that holds an open stream's [`Conn`]. It owns the per-stream `StreamState` map,
//! the cross-task wake primitive ([`AtomicWaker`]), and the [`HttpContext`] / [`Swansong`]
//! the broader server stack reaches in through.
//!
//! The driver loop itself lives in [`super::acceptor`] — see that module for the
//! per-connection state machine and how send / receive concerns are split.
//!
//! [`H2Acceptor`]: super::H2Acceptor

use super::{H2Acceptor, transport::StreamState};
use crate::{Body, Conn, HttpContext};
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
/// Wrapped in an [`Arc`] and held by both the [`H2Acceptor`] driver and every conn task
/// that holds an open stream's [`Conn`]. Per-stream `StreamState`, HPACK encoder state, and
/// connection-level send flow control will accumulate here as later phases land.
#[derive(Debug)]
pub struct H2Connection {
    context: Arc<HttpContext>,
    swansong: Swansong,
    /// Driver-side waker that conn tasks fire whenever they produce work the driver should
    /// act on — the is-reading signal on first `H2Transport::poll_read`, and the
    /// `submit_send` arrival. Single-consumer (the driver); N producers (conn tasks). The
    /// driver registers its current `poll_next` waker here each iteration it parks.
    outbound_waker: AtomicWaker,
    /// Per-stream shared state, keyed by stream id. The driver inserts on stream open and
    /// removes on close. Conn-task-side code (`ReceivedBody`, `Conn::send_h2`) looks up
    /// via private accessor methods on `H2Connection` rather than touching the map
    /// directly — `StreamState` stays module-private. The driver also caches each
    /// `Arc<StreamState>` in its private `StreamEntry` for hot-loop perf, so every entry
    /// here has refcount ≥ 2 while the stream is open.
    streams: Mutex<HashMap<u32, Arc<StreamState>>>,
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

    /// Bind this `H2Connection` to a TCP transport and return an [`H2Acceptor`] that drives
    /// the connection.
    ///
    /// The acceptor must be polled to completion via repeated calls to
    /// [`H2Acceptor::next`] (or [`H2Acceptor::poll_next`]); each returned [`Conn`] should
    /// be spawned on its own task.
    pub fn run<T>(self: Arc<Self>, transport: T) -> H2Acceptor<T>
    where
        T: AsyncRead + AsyncWrite + Unpin + Send,
    {
        H2Acceptor::new(self, transport)
    }

    /// Per-stream entry point — call from the runtime adapter's spawned task for each
    /// [`Conn`] returned by [`H2Acceptor::next`]. Runs `handler` to produce the response,
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
        self: &Arc<Self>,
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
            });
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
pub struct SubmitSend {
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

        if state.send.completed.load(Ordering::Acquire) {
            let result = state
                .send
                .completion_result
                .lock()
                .expect("completion_result mutex poisoned")
                .take()
                .unwrap_or(Ok(()));
            return Poll::Ready(result);
        }

        state.send.completion_waker.register(cx.waker());

        // Re-check after registering so we don't miss a wake fired between the load above
        // and the registration.
        if state.send.completed.load(Ordering::Acquire) {
            let result = state
                .send
                .completion_result
                .lock()
                .expect("completion_result mutex poisoned")
                .take()
                .unwrap_or(Ok(()));
            return Poll::Ready(result);
        }

        Poll::Pending
    }
}
