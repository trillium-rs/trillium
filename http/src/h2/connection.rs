//! HTTP/2 connection driver (RFC 9113).
//!
//! [`H2Connection`] is the shared, [`Arc`]-able per-connection state — handler tasks reference it
//! by way of their [`H2Transport`] to talk back to the driver. [`H2Acceptor`] owns the underlying
//! TCP transport and the demux state, and is driven by the runtime adapter via
//! [`H2Acceptor::next`]: each call returns the next opened request stream (an [`H2Transport`] for
//! the runtime to spawn a handler task against), or `None` when the connection is closed.
//!
//! The driver is a poll-based state machine, not an async fn. A single [`H2Acceptor::poll_next`]
//! call is the unit of forward progress: it drains any pending outbound bytes, advances the read
//! cursor, and dispatches frames as they complete, parking with cancel-safe partial state when
//! no further progress can be made. [`H2Acceptor::next`] is an `async fn` wrapper around
//! `poll_next` for ergonomic use by the runtime adapter.
//!
//! [`H2Transport`]: super::transport::H2Transport

use super::{
    H2Error, H2ErrorCode, H2Settings,
    frame::{self, FRAME_HEADER_LEN, Frame, FrameDecodeError, FrameHeader},
    transport::{H2Transport, StreamState},
};
use crate::{Body, Conn, Headers, HttpContext, headers::hpack::HpackDecoder};
use atomic_waker::AtomicWaker;
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    collections::HashMap,
    future::{Future, poll_fn},
    io,
    pin::Pin,
    sync::{Arc, Mutex, atomic::Ordering},
    task::{Context, Poll, ready},
};
use swansong::{ShutdownCompletion, ShuttingDown, Swansong};

/// The client connection preface (RFC 9113 §3.4). 24 bytes the client MUST send before any
/// HTTP/2 frames.
pub(crate) const CLIENT_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// Upper bound for transient frame buffers — prevents runaway allocation on a peer that advertises
/// an absurd `MAX_FRAME_SIZE`. The per-connection maximum is negotiated via SETTINGS and will
/// replace this in phase 7.
const MAX_BUFFER_SIZE: usize = 1 << 20;

/// Initial HPACK dynamic table size per RFC 7541 §4.2 — also the value implied by an absent
/// `SETTINGS_HEADER_TABLE_SIZE`. Phase 7 will let `HttpConfig` raise or lower this; for now it's
/// hardcoded to match the default we advertise.
const HPACK_TABLE_SIZE: usize = 4096;

/// Per-stream recv flow-control window we top the peer up to once a handler declares intent to
/// consume its request body (via [`H2Transport::poll_read`]). Bounds the peer's in-flight DATA
/// per stream and our per-stream recv buffer footprint. Phase 7 will pull this from
/// `HttpConfig::h2_max_stream_window`; defaulting to 64 KiB matches Chrome / Firefox / hyper.
///
/// We advertise `INITIAL_WINDOW_SIZE = 0` in server SETTINGS — the peer cannot send any body
/// bytes until the driver emits a `WINDOW_UPDATE` for the stream, which it does only after
/// observing the handler's is-reading signal. A handler that never reads its body costs one
/// HEADERS frame and nothing more.
const MAX_STREAM_WINDOW: u32 = 64 * 1024;

/// RFC 9113 §6.5.2 default for `SETTINGS_MAX_FRAME_SIZE`. Used as the per-frame payload cap
/// (HEADERS / CONTINUATION header-block bytes, DATA payload bytes) until peer SETTINGS parsing
/// in step 4 replaces this with the negotiated value.
const DEFAULT_PEER_MAX_FRAME_SIZE: u32 = 16_384;

/// Shared per-connection state for HTTP/2.
///
/// Wrapped in an [`Arc`] and held by both the [`H2Acceptor`] driver and every [`H2Transport`]
/// handed to a handler task. Per-stream tables, HPACK encoder state, and connection-level send
/// flow control will accumulate here as later phases land.
///
/// [`H2Transport`]: super::transport::H2Transport
#[derive(Debug)]
pub struct H2Connection {
    context: Arc<HttpContext>,
    swansong: Swansong,
    /// Driver-side waker that handler tasks fire whenever they produce work the driver should
    /// act on — for now just the is-reading signal on first `H2Transport::poll_read`, in phase
    /// 4 also the `submit_response` arrival. Single-consumer (the driver); N producers (handler
    /// tasks). The driver registers its current `poll_next` waker here each iteration it parks.
    pub(super) outbound_waker: AtomicWaker,
    /// Per-stream shared state, keyed by stream id. The driver inserts on stream open; later
    /// phases will remove on stream close. Conn-task-side code (`ReceivedBody`, `Conn::send_h2`)
    /// looks up via private accessor methods on `H2Connection` rather than touching the map
    /// directly — `StreamState` stays module-private. The driver also caches each
    /// `Arc<StreamState>` in its private `StreamEntry` for hot-loop perf, so every entry here
    /// has refcount ≥ 2 while the stream is open.
    pub(super) streams: Mutex<HashMap<u32, Arc<StreamState>>>,
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

    /// The connection-scoped [`Swansong`]. Shuts down on peer GOAWAY or when the server-level
    /// swansong shuts down.
    pub fn swansong(&self) -> &Swansong {
        &self.swansong
    }

    /// Attempt graceful shutdown of this HTTP/2 connection.
    pub fn shut_down(&self) -> ShutdownCompletion {
        self.swansong.shut_down()
    }

    /// Bind this `H2Connection` to a TCP transport and return an [`H2Acceptor`] that drives the
    /// connection.
    ///
    /// The acceptor must be polled to completion via repeated calls to [`H2Acceptor::next`] (or
    /// [`H2Acceptor::poll_next`]); each returned [`H2Transport`] should be spawned on its own
    /// task.
    ///
    /// [`H2Transport`]: super::transport::H2Transport
    pub fn run<T>(self: Arc<Self>, transport: T) -> H2Acceptor<T>
    where
        T: AsyncRead + AsyncWrite + Unpin + Send,
    {
        H2Acceptor::new(self, transport)
    }

    /// Per-stream entry point — call from the runtime adapter's spawned task for each
    /// [`Conn`] returned by [`H2Acceptor::next`]. Runs `handler` to produce the response, then
    /// `send_h2` to hand the framed response to the driver.
    ///
    /// Mirrors [`H3Connection::process_inbound_bidi`][crate::h3::H3Connection::process_inbound_bidi]'s
    /// role for h3, except the Conn is already built (the acceptor decoded HEADERS and validated
    /// the request before emitting), so this just runs the handler chain and sends.
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
    /// static-or-literal HPACK encoder — no shared state required), takes the response body off
    /// the [`Conn`], and calls this method. The returned future resolves once the driver has
    /// fully framed and flushed HEADERS+CONTINUATION + DATA + (trailing HEADERS or
    /// `END_STREAM`) onto the wire.
    ///
    /// Trailers are not a separate argument: the driver pulls them off the body via
    /// [`Body::trailers`][crate::Body::trailers] once the body is fully drained, mirroring how
    /// h1's send path works.
    ///
    /// # Errors
    ///
    /// The future resolves to an [`io::Error`] if the body's `poll_read` errors, or if the
    /// underlying transport fails partway through the response.
    pub(crate) fn submit_send(
        self: &Arc<Self>,
        stream_id: u32,
        encoded_headers: Vec<u8>,
        body: Option<Body>,
    ) -> SubmitSend {
        let stream = self
            .streams
            .lock()
            .expect("connection streams mutex poisoned")
            .get(&stream_id)
            .cloned();
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
        SubmitSend {
            stream_id,
            stream,
        }
    }
}

/// Future returned by [`H2Connection::submit_send`]; resolves once the driver has fully
/// framed and flushed the submitted response, or with the relevant `io::Error` on failure.
///
/// Holds the per-stream [`StreamState`] Arc (cloned out of the streams map at submit time),
/// not a connection backref + id — so dropping the future doesn't require another map lookup
/// and the conn task's wake registration stays local to the per-stream sync primitives.
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

        // Re-check after registering so we don't miss a wake fired between the load above and
        // the registration.
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

/// Owns the per-connection TCP transport and drives the HTTP/2 demux loop.
///
/// Created by [`H2Connection::run`]. The runtime adapter calls [`Self::next`] (or
/// [`Self::poll_next`] directly) in a loop; each call either returns the next opened request stream
/// (an [`H2Transport`] to be spawned on a handler task) or `None` when the connection is closed.
///
/// Under the hood the driver is a poll-based state machine. `poll_next` is the single
/// forward-progress entry point; `next` wraps it with [`poll_fn`] for async-fn ergonomics.
///
/// [`H2Transport`]: super::transport::H2Transport
#[derive(Debug)]
pub struct H2Acceptor<T> {
    connection: Arc<H2Connection>,
    transport: T,

    /// Overall lifecycle position of the driver.
    state: DriverState,

    /// Future that resolves when the shared `Swansong` begins shutdown. Polled each `poll_next`
    /// tick while the driver is running; on resolution the driver queues a GOAWAY and transitions
    /// to `Closing`, after which the top-of-loop guard returns early and we never poll this again
    /// on the same acceptor.
    shutting_down: ShuttingDown,

    /// Inbound byte cursor. Accumulates bytes from the transport across `poll_next` calls so a
    /// partial frame read can survive a return to `Poll::Pending`. Always contains exactly the
    /// bytes of the current frame being accumulated (header, then payload); reset after each
    /// complete frame is dispatched.
    read_buf: Vec<u8>,
    read_filled: usize,
    read_phase: ReadPhase,

    /// Outbound byte cursor. The driver encodes control frames into `write_buf` and drains to
    /// the transport via `poll_flush_outbound`. `write_cursor` is the offset of the first byte
    /// not yet accepted by `poll_write`. After the buffer fully drains, both fields are reset
    /// and a flush is issued.
    write_buf: Vec<u8>,
    write_cursor: usize,
    write_flush_pending: bool,

    /// HPACK decoder state, shared across all header blocks on this connection.
    hpack: HpackDecoder,

    /// Per-stream state, keyed by stream id. Driver-only — handler tasks hold their own
    /// `Arc<StreamState>` via [`H2Transport`] and don't consult this table. The entry bundles
    /// the shared state with driver-private bookkeeping (e.g. "have we already advertised the
    /// recv window after seeing `is_reading`?").
    streams: HashMap<u32, StreamEntry>,

    /// Highest peer-initiated stream id seen so far. Peer-initiated (client) stream ids must be
    /// odd and strictly increasing per RFC 9113 §5.1.1.
    last_peer_stream_id: u32,

    /// Accumulator for an in-progress HEADERS block that is waiting on further CONTINUATION
    /// frames. `None` outside a HEADERS block. §6.10 forbids any frame on any stream from
    /// interleaving while this is `Some`.
    pending_headers: Option<PendingHeaders>,

    /// Set once the driver decides to close: graceful (peer GOAWAY / server swansong / peer EOF)
    /// or erroring (protocol violation → GOAWAY with code, or I/O failure → no GOAWAY).
    /// `poll_next` completes (returns `Ok(None)` or the error) once outbound drains to empty.
    close_outcome: Option<CloseOutcome>,

    /// Set after `poll_next` yields its terminal result. Subsequent calls return `Ok(None)`
    /// without touching the transport.
    finished: bool,

    /// Peer-advertised `SETTINGS_MAX_FRAME_SIZE`. Caps the payload length of any frame we send.
    /// Defaults to RFC 9113 §6.5.2 (16 KiB); peer SETTINGS parsing in step 4 will update from
    /// the wire.
    peer_max_frame_size: u32,

    /// Reusable scratch the send pump reads body chunks into before framing as DATA. Sized
    /// once at construction to fit a peer-max-frame-size payload; never grows during a tick.
    body_scratch: Vec<u8>,
}

/// Position of the connection in its high-level lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DriverState {
    /// Haven't read the client preface yet.
    AwaitingPreface,
    /// Preface read; need to queue our initial SETTINGS frame to `write_buf`.
    NeedsServerSettings,
    /// Steady state — read frames from the transport and dispatch.
    Running,
    /// GOAWAY has been queued; drain `write_buf` then terminate.
    Closing,
}

/// Where the read cursor is inside the current frame.
#[derive(Debug, Clone, Copy)]
enum ReadPhase {
    /// Not yet read the 9 bytes of the next frame header.
    NeedHeader,
    /// Header read and decoded; still collecting payload bytes. `total` is the full target
    /// fill (`FRAME_HEADER_LEN + payload_len`).
    NeedPayload { header: FrameHeader, total: usize },
}

/// Why the driver is closing — shaped around what the terminal `poll_next` result should be.
#[derive(Debug)]
enum CloseOutcome {
    /// Clean close. `poll_next` returns `Ok(None)`.
    Graceful,
    /// Protocol error. `poll_next` returns `Err(...)`. GOAWAY with this code has been queued.
    Protocol(H2ErrorCode),
    /// I/O error. GOAWAY was NOT queued (transport is untrustworthy). Propagated verbatim.
    Io(io::Error),
}

/// HEADERS + CONTINUATION assembly state.
#[derive(Debug)]
struct PendingHeaders {
    stream_id: u32,
    end_stream: bool,
    assembled: Vec<u8>,
}

/// Driver-side view of a single open stream: the shared state the handler also sees, plus a
/// cache of decisions the driver has made for this stream (which the handler doesn't need to
/// know). Grows as phase 3 / phase 4 add state machine and flow-control bookkeeping.
#[derive(Debug)]
struct StreamEntry {
    /// Shared state (recv buffer, send slot, handler wakers). Owned by `Arc` so the
    /// handler task can outlive or operate concurrently with the driver's view.
    shared: Arc<StreamState>,

    /// `true` once the driver has emitted a `WINDOW_UPDATE` in response to the handler's first
    /// `poll_read` (via `recv.is_reading`). Stops duplicate emissions — every subsequent
    /// `poll_next` scan observes `is_reading == true` but we only top up the window once.
    /// Phase 7's refill-as-handler-drains model will reuse this slot as the live advertised
    /// count rather than a boolean.
    window_advertised: bool,

    /// Driver-private send-side state for an in-progress response. `None` until the conn task
    /// submits a response via [`H2Connection::submit_send`] and the driver picks it up on its
    /// next `service_handler_signals` tick.
    send: Option<SendCursor>,
}

impl StreamEntry {
    fn new(shared: Arc<StreamState>) -> Self {
        Self {
            shared,
            window_advertised: false,
            send: None,
        }
    }
}

/// Driver-private state for an in-progress response on a single stream. Never observed
/// concurrently — only the driver task touches this.
#[derive(Debug)]
struct SendCursor {
    /// Pre-encoded HEADERS bytes (HPACK output from the conn task), chunked into HEADERS +
    /// CONTINUATION frames as `peer_max_frame_size` allows.
    encoded_headers: Vec<u8>,
    /// Offset into `encoded_headers` of the first byte not yet emitted.
    headers_offset: usize,
    /// Whether this stream's response carries a body. Decides whether the final HEADERS
    /// fragment carries `END_STREAM` (no body, no trailers) or whether we transition to
    /// the Body phase next.
    has_body: bool,
    /// Body source. Driver polls `body.poll_read` to fill DATA frames; transitions to None
    /// once drained (a 0-byte read).
    body: Option<Body>,
    /// Trailers, populated from `body.trailers()` once the body is fully drained.
    trailers: Option<Headers>,
    /// Where we are in the response.
    phase: SendPhase,
}

/// Position of a `SendCursor` in the response lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendPhase {
    /// Still emitting HEADERS + CONTINUATION fragments.
    Headers,
    /// HEADERS done; pumping body bytes into DATA frames.
    Body,
    /// Body fully drained; emit trailing HEADERS (if trailers) or empty `DATA(END_STREAM)`.
    Trailers,
    /// Completion has been signaled to the conn task; entry can be cleaned up.
    Complete,
}

/// Result of dispatching one decoded frame.
enum Action {
    /// Frame handled; continue the main loop.
    Continue,
    /// A stream just opened and the request validated — return the [`Conn`] to the caller; the
    /// runtime adapter spawns a handler task per emitted Conn. Boxed to keep the enum small —
    /// `Conn` is over 500 bytes and most dispatches return `Continue`.
    Emit(Box<Conn<H2Transport>>),
    /// Begin graceful or erroring close with this outcome.
    Close(CloseOutcome),
}

impl<T> H2Acceptor<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    fn new(connection: Arc<H2Connection>, transport: T) -> Self {
        let shutting_down = connection.swansong.shutting_down();
        Self {
            connection,
            transport,
            state: DriverState::AwaitingPreface,
            shutting_down,
            read_buf: vec![0u8; FRAME_HEADER_LEN],
            read_filled: 0,
            read_phase: ReadPhase::NeedHeader,
            write_buf: Vec::new(),
            write_cursor: 0,
            write_flush_pending: false,
            hpack: HpackDecoder::new(HPACK_TABLE_SIZE),
            streams: HashMap::new(),
            last_peer_stream_id: 0,
            pending_headers: None,
            close_outcome: None,
            finished: false,
            peer_max_frame_size: DEFAULT_PEER_MAX_FRAME_SIZE,
            body_scratch: vec![0u8; DEFAULT_PEER_MAX_FRAME_SIZE as usize],
        }
    }

    /// The shared [`H2Connection`] this acceptor was created from.
    pub fn connection(&self) -> &Arc<H2Connection> {
        &self.connection
    }

    /// Drive the connection until the next request stream opens, the connection ends, or a fatal
    /// protocol or I/O error occurs.
    ///
    /// Returns `Ok(Some(conn))` for each new request stream — the runtime adapter is expected
    /// to spawn a handler task that consumes the [`Conn`]. Malformed requests (RFC 9113 §8.1.2)
    /// are handled internally with a stream-level `RST_STREAM` and never surfaced. Returns
    /// `Ok(None)` when the connection has been shut down cleanly (peer GOAWAY, our own swansong
    /// shutdown, peer EOF at a frame boundary).
    ///
    /// # Errors
    ///
    /// Returns an [`H2Error`] for any *connection-level* protocol violation detected while
    /// decoding peer frames or for an unrecoverable transport I/O error. A final GOAWAY is sent
    /// before a protocol error is returned (best-effort; I/O errors skip it).
    pub async fn next(&mut self) -> Result<Option<Conn<H2Transport>>, H2Error> {
        poll_fn(|cx| self.poll_next(cx)).await
    }

    /// Poll-based driver core. See [`Self::next`] for the async-fn wrapper and the overall
    /// semantics; the `Poll` shape is available so `select`-style combinators and runtime
    /// adapters can drive the connection directly.
    ///
    /// # Errors
    ///
    /// Same as [`Self::next`].
    pub fn poll_next(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<Conn<H2Transport>>, H2Error>> {
        if self.finished {
            return Poll::Ready(Ok(None));
        }

        loop {
            // 1. Conn-task signals. Picks up window-update intent (`is_reading`) and new
            //    `submit_send` submissions, moving them into driver-private state.
            self.service_handler_signals();

            // 2. Send pump. Turns picked-up SendCursors into HEADERS / DATA / trailing-HEADERS
            //    frame bytes in `write_buf`. Body reads that return Pending leave the cursor in
            //    place — the body's source will wake the driver task when it has bytes.
            self.advance_outbound_sends(cx);

            // 3. Flush any pending outbound — never re-poll reads when we still owe bytes to the
            //    peer, and never signal closure to the caller before the wire is clean.
            match self.poll_flush_outbound(cx) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(e)) => {
                    // Flush failure while closing: just take whatever outcome we had and shelve
                    // the fresh I/O error. While running, record and finish.
                    if self.close_outcome.is_none() {
                        self.close_outcome = Some(CloseOutcome::Io(e));
                    }
                    return Poll::Ready(self.finish_with_current_outcome());
                }
                Poll::Pending => return Poll::Pending,
            }

            // 4. If we were closing, outbound is now drained — we're done.
            if self.state == DriverState::Closing {
                return Poll::Ready(self.finish_with_current_outcome());
            }

            // 5. Server-initiated shutdown check. Post-shutdown re-polls are harmless for this
            //    `ShuttingDown` (event_listener-backed, not single-shot), and begin_close flips us
            //    to `Closing` so the guard above returns before we get here again anyway.
            if Pin::new(&mut self.shutting_down).poll(cx).is_ready() {
                self.begin_close(CloseOutcome::Graceful);
                continue;
            }

            // 6. State-specific step.
            match self.state {
                DriverState::AwaitingPreface => match self.poll_read_preface(cx) {
                    Poll::Ready(Ok(())) => self.state = DriverState::NeedsServerSettings,
                    Poll::Ready(Err(e)) => {
                        self.close_outcome = Some(e);
                        return Poll::Ready(self.finish_with_current_outcome());
                    }
                    Poll::Pending => {
                        if self.park(cx) {
                            return Poll::Pending;
                        }
                    }
                },

                DriverState::NeedsServerSettings => {
                    self.queue_settings();
                    self.state = DriverState::Running;
                }

                DriverState::Running => match self.poll_advance_read(cx) {
                    Poll::Ready(Ok(Action::Continue)) => {}
                    Poll::Ready(Ok(Action::Emit(conn))) => {
                        return Poll::Ready(Ok(Some(*conn)));
                    }
                    Poll::Ready(Ok(Action::Close(outcome))) => {
                        self.begin_close(outcome);
                    }
                    Poll::Ready(Err(e)) => {
                        self.close_outcome = Some(e);
                        return Poll::Ready(self.finish_with_current_outcome());
                    }
                    Poll::Pending => {
                        if self.park(cx) {
                            return Poll::Pending;
                        }
                    }
                },

                DriverState::Closing => unreachable!("handled above once write_buf is drained"),
            }
        }
    }

    /// Register the driver's waker with the shared `outbound_waker` (so handler tasks can
    /// wake the driver) and tell the caller whether it's safe to park. Returns `true` if the
    /// driver should return `Poll::Pending`, or `false` if a handler produced work between our
    /// last check and the registration — in which case the caller should loop around to pick
    /// it up.
    fn park(&mut self, cx: &mut Context<'_>) -> bool {
        self.connection.outbound_waker.register(cx.waker());
        !self.has_pending_handler_signals()
    }

    /// Scan streams for conn-task-side signals that the driver should turn into driver-internal
    /// state. Two signals:
    /// - `recv.is_reading` (lazy `WINDOW_UPDATE`): conn task declared intent to read the
    ///   request body; emit a `WINDOW_UPDATE` topping the per-stream recv window up.
    /// - `send.submission` (response handoff): conn task called `submit_send`; move the
    ///   submission into the driver's private `SendCursor` so the next `advance_outbound_sends`
    ///   tick can start framing.
    ///
    /// Each stream's `StreamEntry` caches whether the corresponding driver-side action has
    /// already happened so we don't re-emit on every scan.
    fn service_handler_signals(&mut self) {
        // Collect stream_ids first to avoid holding &mut self.streams across `queue_*` calls
        // (which take &mut self). Short-lived Vec; bounded by MAX_CONCURRENT_STREAMS.
        let needs_advertise: Vec<u32> = self
            .streams
            .iter_mut()
            .filter_map(|(&id, entry)| {
                (!entry.window_advertised && entry.shared.recv.is_reading.load(Ordering::Acquire))
                    .then(|| {
                        entry.window_advertised = true;
                        id
                    })
            })
            .collect();
        for stream_id in needs_advertise {
            self.queue_window_update(stream_id, MAX_STREAM_WINDOW);
        }

        // Pick up new submissions. Iterate in place — `entry.send` is driver-private, no
        // borrow conflict with `self.write_buf`.
        for entry in self.streams.values_mut() {
            if entry.send.is_some() {
                continue;
            }
            let submission = entry
                .shared
                .send
                .submission
                .lock()
                .expect("send submission mutex poisoned")
                .take();
            if let Some(submission) = submission {
                let has_body = submission.body.is_some();
                entry.send = Some(SendCursor {
                    encoded_headers: submission.encoded_headers,
                    headers_offset: 0,
                    has_body,
                    body: submission.body,
                    trailers: None,
                    phase: SendPhase::Headers,
                });
            }
        }
    }

    /// True if any stream has a conn-task signal pending that we haven't yet serviced. Used by
    /// `park` to decide whether returning `Pending` is safe or whether we need to loop around.
    fn has_pending_handler_signals(&self) -> bool {
        self.streams.values().any(|e| {
            (!e.window_advertised && e.shared.recv.is_reading.load(Ordering::Acquire))
                || e.shared
                    .send
                    .submission
                    .lock()
                    .expect("send submission mutex poisoned")
                    .is_some()
        })
    }

    /// True if any stream has an in-progress `SendCursor` that could make synchronous progress
    /// (i.e. is in a phase that doesn't require new external input). Used to decide whether to
    /// re-enter the main loop after parking.
    fn has_active_sends(&self) -> bool {
        self.streams
            .values()
            .any(|e| e.send.as_ref().is_some_and(|s| s.phase != SendPhase::Body))
    }

    /// Advance every active send by at most one step per tick (headers fragments are emitted
    /// atomically per stream — RFC 9113 §6.10 forbids interleaving HEADERS+CONTINUATION with
    /// any other frame on any other stream). Body reads that return Pending leave the cursor
    /// in place; the body's source will wake the driver task when bytes are available.
    fn advance_outbound_sends(&mut self, cx: &mut Context<'_>) {
        let stream_ids: Vec<u32> = self.streams.keys().copied().collect();
        for stream_id in stream_ids {
            self.advance_one_send(stream_id, cx);
        }
    }

    /// Advance one stream's `SendCursor` by one frame's worth of work, with the §6.10
    /// exception: in `Headers` phase we keep emitting fragments back-to-back until `END_HEADERS`
    /// is set. Other phases emit at most one frame per tick to keep streams roughly fair.
    fn advance_one_send(&mut self, stream_id: u32, cx: &mut Context<'_>) {
        let Some(mut send) = self
            .streams
            .get_mut(&stream_id)
            .and_then(|e| e.send.take())
        else {
            return;
        };

        loop {
            match send.phase {
                SendPhase::Headers => {
                    // §6.10 forbids interleaving HEADERS+CONTINUATION with any other frame,
                    // including frames on other streams. The unconditional loop iteration that
                    // follows keeps emitting fragments while still in Headers, or moves into
                    // the new phase this tick if transitioned (avoiding an extra park cycle).
                    self.emit_one_headers_fragment(stream_id, &mut send);
                }
                SendPhase::Body => match self.poll_emit_one_data(stream_id, &mut send, cx) {
                    Poll::Ready(Ok(())) => {
                        // Body returned Ready(N>0) (emitted DATA, still Body) or Ready(0)
                        // (transitioned to Trailers). On transition, run the new phase this
                        // tick; on stay-in-Body, yield to the next stream.
                        if send.phase == SendPhase::Body {
                            break;
                        }
                    }
                    Poll::Ready(Err(e)) => {
                        self.complete_and_remove_stream(stream_id, Err(e));
                        return;
                    }
                    Poll::Pending => break, // body's source will wake the driver task
                },
                SendPhase::Trailers => {
                    // Always transitions to Complete; the next loop iteration fires it.
                    self.emit_trailers_or_end_stream(stream_id, &mut send);
                }
                SendPhase::Complete => {
                    self.complete_and_remove_stream(stream_id, Ok(()));
                    return;
                }
            }
        }

        // Cursor still active — put it back.
        if let Some(entry) = self.streams.get_mut(&stream_id) {
            entry.send = Some(send);
        }
    }

    /// Signal send completion on the stream's `StreamState`, then remove the stream from both
    /// the driver's private map and `H2Connection.streams`. After this the conn task's pending
    /// `SubmitSend` future will see `completed = true` on its next poll and resolve.
    fn complete_and_remove_stream(&mut self, stream_id: u32, result: io::Result<()>) {
        if let Some(entry) = self.streams.remove(&stream_id) {
            signal_send_completion(&entry.shared, result);
        }
        self.connection
            .streams
            .lock()
            .expect("connection streams mutex poisoned")
            .remove(&stream_id);
    }

    /// Emit one HEADERS or CONTINUATION fragment from `send.encoded_headers`. Transitions
    /// `send.phase` to `Body` / `Trailers` / `Complete` once `END_HEADERS` is set. The first
    /// fragment is HEADERS; subsequent fragments (when `headers_offset > 0`) are CONTINUATION.
    fn emit_one_headers_fragment(&mut self, stream_id: u32, send: &mut SendCursor) {
        let max_payload = self.peer_max_frame_size as usize;
        let remaining = send.encoded_headers.len() - send.headers_offset;
        let chunk_len = remaining.min(max_payload);
        let end_headers = chunk_len == remaining;
        let is_first = send.headers_offset == 0;
        let chunk_len_u32 = u32::try_from(chunk_len).expect("chunk_len <= peer_max_frame_size u32");

        if is_first {
            // Final HEADERS fragment with no body and no trailers carries END_STREAM.
            let end_stream = end_headers && !send.has_body;
            let prefix_len = frame::headers::encoded_prefix_len(0, false);
            let start = self.write_buf.len();
            self.write_buf.resize(start + prefix_len, 0);
            frame::headers::encode_prefix(
                stream_id,
                end_stream,
                end_headers,
                None,
                chunk_len_u32,
                0,
                &mut self.write_buf[start..],
            )
            .expect("buffer sized from encoded_prefix_len");
        } else {
            let prefix_len = frame::continuation::ENCODED_PREFIX_LEN;
            let start = self.write_buf.len();
            self.write_buf.resize(start + prefix_len, 0);
            frame::continuation::encode_prefix(
                stream_id,
                end_headers,
                chunk_len_u32,
                &mut self.write_buf[start..],
            )
            .expect("buffer sized from ENCODED_PREFIX_LEN");
        }

        // Append the header-block fragment payload.
        self.write_buf
            .extend_from_slice(&send.encoded_headers[send.headers_offset..send.headers_offset + chunk_len]);
        send.headers_offset += chunk_len;
        self.write_flush_pending = true;

        if end_headers {
            send.phase = if send.has_body {
                SendPhase::Body
            } else {
                // The single HEADERS fragment carried END_STREAM (or final CONTINUATION did
                // not — but our encoder above only sets END_STREAM on the *first* fragment, so
                // for the multi-fragment + no-body case we'd need an extra empty DATA. That
                // case is unreachable today: response headers always fit comfortably in one
                // peer-default 16 KiB frame, but still — guard with a Trailers transition that
                // the next tick will turn into an empty DATA(END_STREAM).
                if is_first {
                    SendPhase::Complete
                } else {
                    SendPhase::Trailers
                }
            };
        }
    }

    /// Poll the body for one DATA chunk. On `Ready(Ok(0))`, takes trailers off the body and
    /// transitions to `Trailers`. On `Ready(Ok(n))`, emits one DATA frame (no `END_STREAM`).
    /// On `Pending`, the cursor stays in `Body` — body's source will wake the driver task.
    fn poll_emit_one_data(
        &mut self,
        stream_id: u32,
        send: &mut SendCursor,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        let Some(body) = send.body.as_mut() else {
            // Body already drained but somehow we're still in Body phase — treat as 0-byte EOF.
            send.phase = SendPhase::Trailers;
            return Poll::Ready(Ok(()));
        };

        let n = ready!(Pin::new(body).poll_read(cx, &mut self.body_scratch))?;
        if n == 0 {
            // Body drained. Take trailers off it, drop the body, transition.
            send.trailers = send.body.as_mut().and_then(Body::trailers);
            send.body = None;
            send.phase = SendPhase::Trailers;
            return Poll::Ready(Ok(()));
        }

        let n_u32 = u32::try_from(n).expect("read n <= peer_max_frame_size u32");
        let prefix_len = frame::data::encoded_prefix_len(0);
        let start = self.write_buf.len();
        self.write_buf.resize(start + prefix_len, 0);
        frame::data::encode_prefix(
            stream_id,
            false, // never END_STREAM here; trailers / empty-DATA carries END_STREAM
            n_u32,
            0,
            &mut self.write_buf[start..],
        )
        .expect("buffer sized from encoded_prefix_len");
        self.write_buf
            .extend_from_slice(&self.body_scratch[..n]);
        self.write_flush_pending = true;
        Poll::Ready(Ok(()))
    }

    /// Emit either a trailing HEADERS (with `END_STREAM`) if the response has trailers, or an
    /// empty `DATA(END_STREAM)` frame as the stream terminator. Transitions to `Complete` so the
    /// next tick fires the conn-task completion signal.
    fn emit_trailers_or_end_stream(&mut self, stream_id: u32, send: &mut SendCursor) {
        if let Some(trailers) = send.trailers.take() {
            // Encode trailers via the static-or-literal HPACK encoder. Trailers carry no
            // pseudo-headers (response status/etc. are already in the HEADERS frame).
            let mut block = Vec::new();
            crate::headers::hpack::encode(
                &crate::headers::hpack::FieldSection::new(
                    crate::headers::hpack::PseudoHeaders::default(),
                    &trailers,
                ),
                &mut block,
            );
            // Trailing HEADERS: END_HEADERS=true, END_STREAM=true.
            let block_len_u32 = u32::try_from(block.len()).expect("trailers block fits u32");
            let prefix_len = frame::headers::encoded_prefix_len(0, false);
            let start = self.write_buf.len();
            self.write_buf.resize(start + prefix_len, 0);
            frame::headers::encode_prefix(
                stream_id,
                true,
                true,
                None,
                block_len_u32,
                0,
                &mut self.write_buf[start..],
            )
            .expect("buffer sized from encoded_prefix_len");
            self.write_buf.extend_from_slice(&block);
        } else {
            // No trailers — empty DATA frame with END_STREAM as the stream terminator.
            let prefix_len = frame::data::encoded_prefix_len(0);
            let start = self.write_buf.len();
            self.write_buf.resize(start + prefix_len, 0);
            frame::data::encode_prefix(stream_id, true, 0, 0, &mut self.write_buf[start..])
                .expect("buffer sized from encoded_prefix_len");
        }
        self.write_flush_pending = true;
        send.phase = SendPhase::Complete;
    }

    /// Convert the current `close_outcome` into the terminal return of `poll_next`. Must only be
    /// called after outbound bytes have been flushed.
    fn finish_with_current_outcome(&mut self) -> Result<Option<Conn<H2Transport>>, H2Error> {
        self.finished = true;
        match self.close_outcome.take() {
            None | Some(CloseOutcome::Graceful) => Ok(None),
            Some(CloseOutcome::Protocol(code)) => Err(H2Error::Protocol(code)),
            Some(CloseOutcome::Io(e)) => Err(H2Error::Io(e)),
        }
    }

    /// Enter the closing state: record the outcome and queue a GOAWAY (only for outcomes that
    /// warrant one). The main loop will drain `write_buf` and then finish.
    fn begin_close(&mut self, outcome: CloseOutcome) {
        // Don't overwrite a prior outcome (e.g. if an error fires in the middle of a graceful
        // shutdown, keep the error).
        let code = match &outcome {
            CloseOutcome::Graceful => Some(H2ErrorCode::NoError),
            CloseOutcome::Protocol(code) => Some(*code),
            CloseOutcome::Io(_) => None,
        };
        if self.close_outcome.is_none() {
            self.close_outcome = Some(outcome);
        }
        if let Some(code) = code {
            self.queue_goaway(self.last_peer_stream_id, code);
        }
        self.state = DriverState::Closing;
    }

    /// Advance the read side by one frame. Accumulates bytes, and once a complete frame is
    /// available, dispatches it and returns the resulting action.
    ///
    /// Always returns after handling one frame (even on `Action::Continue`) so the outer loop
    /// gets a chance to flush any outbound bytes that dispatch queued — holding them in
    /// `write_buf` across reads would deadlock against a peer that's waiting for an ACK before
    /// sending its next frame.
    fn poll_advance_read(&mut self, cx: &mut Context<'_>) -> Poll<Result<Action, CloseOutcome>> {
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

    /// Read bytes from the transport into `read_buf[read_filled..target]` until `read_filled >=
    /// target`. Cancel-safe: if the caller drops the Future, any bytes already placed are
    /// preserved in the buffer.
    ///
    /// A 0-byte read is surfaced as `UnexpectedEof`. The caller maps this to a terminal I/O
    /// error; we don't emit a GOAWAY on peer-initiated close (consistent with the pre-poll
    /// driver).
    fn poll_fill_to(&mut self, target: usize, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.read_buf.len() < target {
            self.read_buf.resize(target, 0);
        }
        while self.read_filled < target {
            let n = ready!(
                Pin::new(&mut self.transport)
                    .poll_read(cx, &mut self.read_buf[self.read_filled..target])
            )?;
            if n == 0 {
                return Poll::Ready(Err(io::Error::from(io::ErrorKind::UnexpectedEof)));
            }
            self.read_filled += n;
        }
        Poll::Ready(Ok(()))
    }

    /// Read the 24-byte client connection preface (§3.4) and validate it. Uses `read_buf` /
    /// `read_filled` so a partial preface survives a return to `Poll::Pending`.
    fn poll_read_preface(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), CloseOutcome>> {
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

    /// Drain `write_buf[write_cursor..]` to the transport, then flush if bytes were written.
    /// Returns `Ready(Ok(()))` when both the buffer is empty AND any pending flush has completed.
    fn poll_flush_outbound(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.write_cursor < self.write_buf.len() {
            let n = ready!(
                Pin::new(&mut self.transport).poll_write(cx, &self.write_buf[self.write_cursor..])
            )?;
            if n == 0 {
                return Poll::Ready(Err(io::Error::from(io::ErrorKind::WriteZero)));
            }
            self.write_cursor += n;
        }
        // Fully drained — reset the buffer so future writes start at offset 0.
        self.write_buf.clear();
        self.write_cursor = 0;
        if self.write_flush_pending {
            ready!(Pin::new(&mut self.transport).poll_flush(cx))?;
            self.write_flush_pending = false;
        }
        Poll::Ready(Ok(()))
    }

    /// Decoded frame arrived — run the connection-level side-effects.
    ///
    /// `payload_start` is the offset within `self.read_buf` where the frame's body bytes begin
    /// (past the fixed header and any per-frame prefix — same value `Frame::decode` returned).
    /// `total` is the full `FRAME_HEADER_LEN + payload_len` so header-block / data consumers can
    /// slice against it.
    fn dispatch(
        &mut self,
        frame: Frame,
        payload_start: usize,
        total: usize,
    ) -> Result<Action, CloseOutcome> {
        // §6.10: while a HEADERS block is in progress (pending_headers.is_some()), the ONLY
        // frame the peer may send on any stream is the matching CONTINUATION. Anything else is
        // a connection-level PROTOCOL_ERROR.
        if self.pending_headers.is_some() && !matches!(frame, Frame::Continuation { .. }) {
            return Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError));
        }

        match frame {
            Frame::Settings(_) => {
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
                self.connection.swansong.shut_down();
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
            // §6.6 PUSH_PROMISE from a client is a connection error; §6.10 CONTINUATION without
            // an in-progress header block is too (but pending_headers==Some is handled via the
            // match arm above).
            Frame::PushPromise { .. } => Err(CloseOutcome::Protocol(H2ErrorCode::ProtocolError)),
            // Benign frames whose effect isn't yet implemented. Tolerate to keep the handshake
            // clean until the relevant phases.
            Frame::SettingsAck
            | Frame::Ping { ack: true, .. }
            | Frame::WindowUpdate { .. }
            | Frame::RstStream { .. }
            | Frame::Priority { .. }
            | Frame::Unknown { .. } => Ok(Action::Continue),
        }
    }

    /// A HEADERS frame arrived. Either `END_HEADERS` is set (emit the stream immediately) or we
    /// accumulate the fragment into `pending_headers` and wait for CONTINUATION.
    fn handle_headers(
        &mut self,
        stream_id: u32,
        end_stream: bool,
        end_headers: bool,
        header_block_length: u32,
        payload_start: usize,
        total: usize,
    ) -> Result<Action, CloseOutcome> {
        // §5.1.1: a peer-initiated stream id must be odd and strictly greater than every prior
        // peer-initiated stream id, and not already known.
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
    /// On a §8.1.2 malformed-request rejection from `new_h2`, the stream is removed from both
    /// maps, a `RST_STREAM(PROTOCOL_ERROR)` is queued, and `Action::Continue` is returned —
    /// the malformed request never reaches a handler task. (HPACK decode failures, by contrast,
    /// are connection-level: the dynamic table state is now untrustworthy for *every* future
    /// stream on this connection.)
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
        self.connection
            .streams
            .lock()
            .expect("connection streams mutex poisoned")
            .insert(stream_id, state.clone());
        self.streams
            .insert(stream_id, StreamEntry::new(state.clone()));
        self.last_peer_stream_id = stream_id;

        // No eager WINDOW_UPDATE: we advertise `INITIAL_WINDOW_SIZE = 0` in SETTINGS, so the peer
        // cannot send body bytes until the handler calls `H2Transport::poll_read` and the driver
        // observes `recv.is_reading` on a subsequent poll.

        let transport = H2Transport::new(self.connection.clone(), stream_id, state);
        match Conn::new_h2(self.connection.clone(), stream_id, field_section, transport) {
            Ok(conn) => Ok(Action::Emit(Box::new(conn))),
            Err(code) => {
                log::debug!("h2 stream {stream_id}: rejected during build: {code:?}");
                self.streams.remove(&stream_id);
                self.connection
                    .streams
                    .lock()
                    .expect("connection streams mutex poisoned")
                    .remove(&stream_id);
                self.queue_rst_stream(stream_id, code);
                Ok(Action::Continue)
            }
        }
    }

    /// A DATA frame arrived — copy its payload into the matching stream's recv buffer and wake
    /// the handler. Padding bytes are part of the already-read frame body and are skipped
    /// (they're in the buffer but not pushed).
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

    // --- outbound frame queuing helpers -------------------------------------------------------
    //
    // All `queue_*` helpers append encoded bytes to `write_buf` and set `write_flush_pending`.
    // The driver's main loop drains `write_buf` before observing progress elsewhere.

    fn queue_settings(&mut self) {
        let settings = H2Settings::server_defaults();
        let start = self.write_buf.len();
        self.write_buf
            .resize(start + frame::settings::encoded_len(&settings), 0);
        let n = frame::settings::encode(&settings, &mut self.write_buf[start..])
            .expect("buffer sized from encoded_len");
        self.write_buf.truncate(start + n);
        self.write_flush_pending = true;
    }

    fn queue_settings_ack(&mut self) {
        let start = self.write_buf.len();
        self.write_buf
            .resize(start + frame::settings::ACK_ENCODED_LEN, 0);
        frame::settings::encode_ack(&mut self.write_buf[start..])
            .expect("ACK_ENCODED_LEN is exactly the fixed ack size");
        self.write_flush_pending = true;
    }

    fn queue_ping_ack(&mut self, opaque_data: [u8; 8]) {
        let start = self.write_buf.len();
        self.write_buf.resize(start + frame::ping::ENCODED_LEN, 0);
        frame::ping::encode(opaque_data, true, &mut self.write_buf[start..])
            .expect("ENCODED_LEN matches fixed ping size");
        self.write_flush_pending = true;
    }

    fn queue_window_update(&mut self, stream_id: u32, increment: u32) {
        let start = self.write_buf.len();
        self.write_buf
            .resize(start + frame::window_update::ENCODED_LEN, 0);
        frame::window_update::encode(stream_id, increment, &mut self.write_buf[start..])
            .expect("ENCODED_LEN matches fixed window_update size");
        self.write_flush_pending = true;
    }

    fn queue_goaway(&mut self, last_stream_id: u32, code: H2ErrorCode) {
        let start = self.write_buf.len();
        self.write_buf
            .resize(start + frame::goaway::encoded_len(0), 0);
        let n = frame::goaway::encode(last_stream_id, code, &[], &mut self.write_buf[start..])
            .expect("buffer sized from encoded_len");
        self.write_buf.truncate(start + n);
        self.write_flush_pending = true;
    }

    fn queue_rst_stream(&mut self, stream_id: u32, code: H2ErrorCode) {
        let start = self.write_buf.len();
        self.write_buf
            .resize(start + frame::rst_stream::ENCODED_LEN, 0);
        frame::rst_stream::encode(stream_id, code, &mut self.write_buf[start..])
            .expect("ENCODED_LEN matches fixed rst_stream size");
        self.write_flush_pending = true;
    }
}

/// Store the send result on `StreamState`, flip `completed`, wake the conn-task waker. Lone
/// free fn so the driver can call it from inside an `&mut self` borrow chain without a re-lookup.
fn signal_send_completion(state: &StreamState, result: io::Result<()>) {
    *state
        .send
        .completion_result
        .lock()
        .expect("completion_result mutex poisoned") = Some(result);
    state.send.completed.store(true, Ordering::Release);
    state.send.completion_waker.wake();
}

/// Slice the interesting bytes out of a just-read frame. Bounds-checks to defend against a
/// payload length on the wire that disagrees with a body-bearing frame's declared inner length.
fn frame_slice(buf: &[u8], start: usize, length: u32, total: usize) -> Result<&[u8], CloseOutcome> {
    let length =
        usize::try_from(length).map_err(|_| CloseOutcome::Protocol(H2ErrorCode::FrameSizeError))?;
    let end = start
        .checked_add(length)
        .ok_or(CloseOutcome::Protocol(H2ErrorCode::FrameSizeError))?;
    if end > total {
        return Err(CloseOutcome::Protocol(H2ErrorCode::FrameSizeError));
    }
    Ok(&buf[start..end])
}

/// Convert a transport I/O error into a close outcome. Plain I/O errors terminate the driver
/// without emitting a GOAWAY — matching the pre-poll driver's behavior of surfacing `read_exact`
/// EOF as a terminal `H2Error::Io`.
fn io_to_outcome(e: io::Error) -> CloseOutcome {
    CloseOutcome::Io(e)
}
