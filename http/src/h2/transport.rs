//! Per-stream transport handed to handler tasks.
//!
//! [`H2Transport`] is the [`AsyncRead`] + [`AsyncWrite`] view of a single HTTP/2 stream. It is
//! carried on the emitted [`Conn`][crate::Conn] returned from [`H2Driver::next`], and the
//! runtime adapter spawns a handler task that consumes it. The transport never touches the
//! underlying TCP connection directly — all I/O coordinates through shared per-stream state
//! on the [`H2Connection`] driven by the driver task.
//!
//! Two paths reach the impls:
//!
//! - **Normal HTTP/2 request/response**: handlers usually don't touch [`H2Transport`]
//!   directly (same sharp edge h1 and h3 document). [`ReceivedBody`][crate::ReceivedBody]
//!   reads request body bytes through the transport's `AsyncRead` via
//!   [`ReceivedBody::handle_h2_data`][crate::ReceivedBody::handle_h2_data]. Response bytes
//!   flow through [`H2Connection::submit_send`][submit_send] to the driver's send pump,
//!   which frames HEADERS + DATA + trailing HEADERS onto the connection without ever
//!   touching this `AsyncWrite`.
//!
//! - **Extended-CONNECT upgrades** ([RFC 8441] WebSocket-over-h2, plus the in-progress
//!   `draft-ietf-webtrans-http2` for WebTransport-over-h2): after the handler responds 200
//!   to a `CONNECT` request with a `:protocol` pseudo-header,
//!   [`Conn::send_h2`][crate::Conn::send_h2] routes through
//!   [`H2Connection::submit_upgrade`][submit_upgrade] which frames HEADERS without
//!   `END_STREAM`, signals send completion early, and leaves the stream open as a
//!   bidirectional byte channel. The runtime adapter then dispatches
//!   [`Handler::upgrade`][trillium::Handler::upgrade], which gets an
//!   [`Upgrade`][crate::Upgrade] wrapping this transport. `AsyncWrite::poll_write`
//!   appends to a per-stream outbound queue ([`SendState::outbound`]); the driver's send
//!   pump drains it into DATA frames bounded by the per-stream and connection send
//!   windows. `AsyncWrite::poll_close` flips [`SendState::outbound_close_requested`] so
//!   the driver eventually emits `DATA(END_STREAM)` and tears the stream down.
//!
//! [`H2Driver::next`]: super::H2Driver::next
//! [`H2Connection`]: super::H2Connection
//! [`BoxedTransport`]: crate::transport::BoxedTransport
//! [submit_send]: super::H2Connection::submit_send
//! [submit_upgrade]: super::H2Connection::submit_upgrade
//! [RFC 8441]: https://www.rfc-editor.org/rfc/rfc8441

use super::{H2Connection, H2ErrorCode};
use crate::{Body, Buffer, Headers};
use atomic_waker::AtomicWaker;
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    fmt, io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll},
};

/// A single HTTP/2 stream's transport handle.
///
/// Carries a backref to the shared [`H2Connection`], the stream id, and the per-stream
/// `Arc<StreamState>` used by the read side. Normal HTTP/2 operation reads through
/// [`ReceivedBody`][crate::ReceivedBody] and writes through
/// [`H2Connection::submit_send`][super::H2Connection::submit_send]; the `AsyncRead` /
/// `AsyncWrite` impls here are only reached by code that borrows the transport directly
/// (typically an upgrade handler after extended CONNECT).
pub struct H2Transport {
    connection: Arc<H2Connection>,
    stream_id: u32,
    state: Arc<StreamState>,
}

impl fmt::Debug for H2Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("H2Transport")
            .field("stream_id", &self.stream_id)
            .finish_non_exhaustive()
    }
}

impl H2Transport {
    /// Create a transport for a stream that has just been opened by the driver.
    pub(super) fn new(
        connection: Arc<H2Connection>,
        stream_id: u32,
        state: Arc<StreamState>,
    ) -> Self {
        Self {
            connection,
            stream_id,
            state,
        }
    }

    /// The stream identifier this transport is bound to.
    pub fn stream_id(&self) -> u32 {
        self.stream_id
    }

    /// The shared [`H2Connection`] backing this stream.
    pub fn connection(&self) -> &Arc<H2Connection> {
        &self.connection
    }
}

impl AsyncRead for H2Transport {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if out.is_empty() {
            return Poll::Ready(Ok(0));
        }

        // The first `poll_read` is the handler's declaration of intent to consume the request
        // body — until this point, we've advertised a zero recv window and the peer has sent
        // nothing beyond HEADERS. Tell the driver to top up our per-stream window now. Later
        // calls CAS-fail silently and don't re-signal.
        let recv_state = &self.state.recv;
        let connection = &*self.connection;
        if !recv_state.is_reading.swap(true, Ordering::AcqRel) {
            connection.outbound_waker().wake();
        }

        let mut recv = recv_state.buf.lock().expect("recv buf mutex poisoned");

        // Copy as many bytes as fit from the front of the ring into `out`, then advance the
        // ring's virtual read cursor. `Buffer::ignore_front` truncates the underlying `Vec` to
        // zero when we drain fully, so capacity stays bounded by peak in-flight bytes rather
        // than cumulative traffic.
        let take = out.len().min(recv.len());
        if take > 0 {
            out[..take].copy_from_slice(&recv[..take]);
            recv.ignore_front(take);
            // Drop the buf lock before the waker fire so the driver can grab it without
            // contention when it wakes.
            drop(recv);
            // Tell the driver how many bytes the handler consumed so it can emit a matching
            // `WINDOW_UPDATE` and keep the peer's stream + connection windows topped up.
            // `fetch_add` accumulates across calls that happen before the driver's next
            // service tick; the driver's `swap(0)` takes the whole batch at once.
            recv_state
                .bytes_consumed
                .fetch_add(take as u64, Ordering::AcqRel);
            connection.outbound_waker().wake();
            return Poll::Ready(Ok(take));
        }

        // Buffer empty. EOF if END_STREAM was observed, otherwise register and wait.
        // The driver acquires the same `buf` lock to push data and to set `eof`, so holding
        // it here is enough to make the eof check final — no register-then-check race window
        // between us and the driver's wake.
        if recv_state.eof.load(Ordering::Acquire) {
            return Poll::Ready(Ok(0));
        }
        recv_state.waker.register(cx.waker());
        Poll::Pending
    }
}

impl AsyncWrite for H2Transport {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        // Append into the per-stream outbound queue used by the extended-CONNECT
        // (RFC 8441) upgrade path. The driver's send pump drains the same
        // queue (via the upgrade body's `AsyncRead::poll_read`) into DATA frames bounded
        // by per-stream + connection send windows.
        //
        // No backpressure here yet — the queue is unbounded. If a runaway handler
        // becomes a problem we'll cap it and return `Poll::Pending` past the cap, with a
        // wake from the driver's drain side.
        let send = &self.state.send;

        if send.outbound_close_requested.load(Ordering::Acquire) {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }

        let n = buf.len();
        log::trace!(
            "h2 stream {}: H2Transport::poll_write appending {n} bytes to outbound queue",
            self.stream_id,
        );
        send.outbound
            .lock()
            .expect("outbound buf mutex poisoned")
            .extend_from_slice(buf);

        // Wake the driver task (if parked on the connection-level waker) and the
        // upgrade body's poll_read (in case it's registered between driver ticks).
        // Firing both is cheap and resolves the cross-task race where the driver
        // happens to be parked on `connection.outbound_waker` rather than mid-body-poll.
        send.outbound_waker.wake();
        self.connection.outbound_waker().wake();
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Best-effort: bytes appended via `poll_write` are already visible to the driver
        // and will be framed on the next tick. There's no application-level "flushed"
        // state below us to wait on.
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Mark the upgrade write-half closed. Once the driver drains the remaining
        // outbound bytes, the upgrade body's `poll_read` will return `Ready(0)`, the
        // send pump transitions through trailers (none) into `DATA(END_STREAM)`, and the
        // stream tears down via the normal `complete_and_remove_stream` path.
        log::trace!(
            "h2 stream {}: H2Transport::poll_close marking outbound closed",
            self.stream_id,
        );
        self.state
            .send
            .outbound_close_requested
            .store(true, Ordering::Release);
        self.state.send.outbound_waker.wake();
        self.connection.outbound_waker().wake();
        Poll::Ready(Ok(()))
    }
}

/// Shared per-stream state. Owned by an [`Arc`] held jointly by the driver (via the connection's
/// stream table) and the handler task (via [`H2Transport`]).
#[derive(Debug, Default)]
pub(super) struct StreamState {
    /// Recv side: inbound DATA payloads, EOF flag, handler waker, handler-intent signal.
    pub(super) recv: RecvState,
    /// Send side: handoff slot from the conn task's `submit_send`, plus completion signaling
    /// the conn task awaits.
    pub(super) send: SendState,
    /// Stream-error request raised from the conn-task side. Populated by
    /// [`H2Connection::stream_error`][super::H2Connection::stream_error] when something on
    /// the conn-task side (a body-read that detects content-length mismatch, a handler
    /// that wants to abort) needs the driver to emit `RST_STREAM` and clean up. The driver
    /// picks this up in `service_handler_signals` on its next tick and routes through
    /// [`H2Driver::complete_and_remove_stream`][super::H2Driver]'s normal cleanup path.
    ///
    /// [`H2Driver`]: super::H2Driver
    pub(super) pending_reset: Mutex<Option<H2ErrorCode>>,
}

/// Receive-side per-stream state.
#[derive(Debug, Default)]
pub(super) struct RecvState {
    /// Inbound DATA body bytes awaiting handler read. A single persistent ring (append-at-tail,
    /// `ignore_front`-at-head): the driver appends via `extend_from_slice` when a DATA frame
    /// arrives; the handler reads from the front and virtually drops consumed bytes. When
    /// `ignore_front` catches up to the data end the `Buffer` truncates to zero, so the
    /// underlying `Vec` capacity stays bounded by peak in-flight bytes rather than cumulative
    /// traffic — zero amortized allocations per DATA frame.
    pub(super) buf: Mutex<Buffer>,

    /// `true` once `END_STREAM` has been observed for this stream's recv side. Set by the
    /// driver under the same `buf` lock used for pushes; checked by `poll_read` while
    /// holding that lock to decide between EOF and Pending.
    pub(super) eof: AtomicBool,

    /// Handler-task waker, fired by the driver after pushing DATA into `buf` or after
    /// setting `eof`. Single-waiter: only one task ever polls a given `H2Transport`.
    pub(super) waker: AtomicWaker,

    /// Set by the handler's first [`H2Transport::poll_read`] to declare intent to consume the
    /// request body. The driver observes the transition and emits a `WINDOW_UPDATE` for this
    /// stream, topping its recv window up from `SETTINGS_INITIAL_WINDOW_SIZE` (advertised as
    /// `0`) to the per-stream maximum. Once set, stays set.
    pub(super) is_reading: AtomicBool,

    /// Bytes the handler has consumed from `buf` since the driver last sampled this counter.
    /// Incremented by [`H2Transport::poll_read`] using `fetch_add` after each drain; the
    /// driver reads it via `swap(0)` on each tick and emits stream-level + connection-level
    /// `WINDOW_UPDATE` for the consumed total. Ensures a handler draining a body larger than
    /// a single window doesn't stall the peer.
    pub(super) bytes_consumed: AtomicU64,

    /// Trailers, populated by the driver if a trailing HEADERS frame arrives for this stream.
    /// Always written *before* `eof` is set, so once the handler observes `Ready(0)` on the
    /// recv side, any trailers for this request are guaranteed to be in place.
    ///
    /// Taken out and moved into [`Conn::request_trailers`][crate::Conn] by the receiver-side
    /// body state machine when it transitions to
    /// [`ReceivedBodyState::End`][crate::received_body::ReceivedBodyState].
    pub(super) trailers: Mutex<Option<Headers>>,
}

/// Send-side per-stream state used to hand a response from the conn task to the driver,
/// plus the outbound byte queue for extended-CONNECT upgraded streams.
///
/// **Normal response path**: the conn task fills `submission` once via
/// [`H2Connection::submit_send`][submit] and waits on `completion_waker` for `completed` to
/// flip. The driver picks up the submission on its next `drive` tick, frames it (HEADERS,
/// DATA, optional trailing HEADERS) into the connection's outbound buffer as send-side flow
/// control allows, and on completion stores the `completion_result`, sets `completed = true`,
/// and wakes the conn task.
///
/// **Extended-CONNECT upgrade path** ([RFC 8441]): the conn task calls
/// [`H2Connection::submit_upgrade`][submit_upgrade], which constructs an
/// [`H2OutboundReader`] over `outbound` / `outbound_close_requested` /
/// `outbound_waker` and submits it as the response body. The driver signals
/// `completion_waker` as soon as the response HEADERS frame is on the wire (instead of
/// waiting for the body to drain), so the conn task's `submit_upgrade().await` returns and
/// the runtime adapter can dispatch [`Handler::upgrade`][trillium::Handler::upgrade]. The
/// upgrade handler then writes through [`H2Transport`]'s `AsyncWrite`, which appends to
/// `outbound`; the driver's send pump pulls those bytes out via the body's `AsyncRead`
/// and frames them as DATA. Closing the transport sets `outbound_close_requested`, the
/// reader returns `Ready(0)`, and the send pump terminates the stream with
/// `DATA(END_STREAM)`.
///
/// [submit]: super::H2Connection::submit_send
/// [submit_upgrade]: super::H2Connection::submit_upgrade
/// [RFC 8441]: https://www.rfc-editor.org/rfc/rfc8441
#[derive(Debug, Default)]
pub(super) struct SendState {
    /// Slot for the conn task's submission. Some between `submit_send` and the driver's
    /// pickup tick; None at all other times.
    pub(super) submission: Mutex<Option<Submission>>,

    /// Set to `true` by the driver once the response has been fully framed, flushed, or
    /// errored. The conn task's `SubmitSend` future polls this atomic and registers on
    /// `completion_waker`.
    pub(super) completed: AtomicBool,

    /// The driver writes the final result here before flipping `completed`. The conn task
    /// takes it once `completed` is observed true.
    pub(super) completion_result: Mutex<Option<io::Result<()>>>,

    /// The conn task's waker, registered by `SubmitSend::poll` and fired by the driver
    /// after `completed` is set.
    pub(super) completion_waker: AtomicWaker,

    /// Outbound bytes for an extended-CONNECT (RFC 8441) upgraded stream.
    /// Appended to by [`H2Transport`]'s `AsyncWrite::poll_write` and drained by the
    /// upgrade body's `AsyncRead::poll_read` (the driver-task side of the send pump).
    /// Empty for normal responses — the driver pumps the response [`Body`] directly.
    pub(super) outbound: Mutex<Buffer>,

    /// Set by [`H2Transport::poll_close`] to mark the upgrade-side write half closed.
    /// The upgrade body's `poll_read` returns `Ready(0)` once `outbound` is empty and
    /// this flag is set, which transitions the driver's send pump into the
    /// trailers/`DATA(END_STREAM)` phase.
    pub(super) outbound_close_requested: AtomicBool,

    /// Waker for the upgrade body's `poll_read`. Fired by [`H2Transport::poll_write`]
    /// after appending bytes and by [`H2Transport::poll_close`] after flipping
    /// `outbound_close_requested`. Registered by the body during its `poll_read` when
    /// it observes an empty buffer and no close flag.
    pub(super) outbound_waker: AtomicWaker,
}

/// `AsyncRead` source the driver uses as the response body for an extended-CONNECT upgrade.
///
/// Reads from [`SendState::outbound`] — the same per-stream queue [`H2Transport`]'s
/// `AsyncWrite::poll_write` appends to. Returns `Ready(0)` once the queue is empty and
/// [`SendState::outbound_close_requested`] has been set (handler dropped or called
/// `poll_close` on the transport), at which point the driver's send pump transitions
/// through trailers (none) into `DATA(END_STREAM)` and tears the stream down.
///
/// Constructed by [`H2Connection::submit_upgrade`][super::H2Connection::submit_upgrade];
/// wrapped in [`Body::new_streaming`] so the existing send pump can pump it as if it were
/// any other unknown-length response body.
#[derive(Debug)]
pub(super) struct H2OutboundReader {
    state: Arc<StreamState>,
    stream_id: u32,
}

impl H2OutboundReader {
    pub(super) fn new(state: Arc<StreamState>, stream_id: u32) -> Self {
        Self { state, stream_id }
    }
}

impl AsyncRead for H2OutboundReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if out.is_empty() {
            return Poll::Ready(Ok(0));
        }

        let send = &self.state.send;
        let mut outbound = send.outbound.lock().expect("outbound buf mutex poisoned");
        let take = out.len().min(outbound.len());
        if take > 0 {
            out[..take].copy_from_slice(&outbound[..take]);
            outbound.ignore_front(take);
            log::trace!(
                "h2 stream {}: H2OutboundReader::poll_read drained {take} bytes",
                self.stream_id,
            );
            return Poll::Ready(Ok(take));
        }

        // Queue empty. Register first, then re-check the close flag. This closes the
        // register-then-check race against `poll_close` (which doesn't take the buf
        // lock — it just stores the flag and fires the waker). Holding the buf lock
        // means `poll_write` can't race here; only `poll_close` can.
        send.outbound_waker.register(cx.waker());

        if send.outbound_close_requested.load(Ordering::Acquire) {
            log::trace!(
                "h2 stream {}: H2OutboundReader::poll_read EOF (close_requested + empty)",
                self.stream_id,
            );
            return Poll::Ready(Ok(0));
        }
        Poll::Pending
    }
}

/// What the conn task hands the driver to begin a send on a stream.
///
/// `body` carries either a normal response body or, for extended-CONNECT (RFC 8441)
/// upgrades, a streaming body that reads from [`SendState::outbound`] (which the upgrade
/// handler's [`H2Transport`] `AsyncWrite` writes into). Trailers (if any) come from
/// [`Body::trailers`] after drain — not a separate field.
///
/// `is_upgrade` flips the driver's completion semantics: instead of signaling
/// [`SubmitSend`][super::SubmitSend] completion after the body is fully on the wire, the
/// driver signals completion as soon as the response HEADERS frame is flushed. That lets
/// [`Conn::send_h2`][crate::Conn::send_h2] return so the runtime can dispatch
/// [`Handler::upgrade`][trillium::Handler::upgrade], while the body keeps streaming in the
/// background.
#[derive(Debug)]
pub(super) struct Submission {
    pub(super) encoded_headers: Vec<u8>,
    pub(super) body: Option<Body>,
    pub(super) is_upgrade: bool,
}
