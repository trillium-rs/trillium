//! Per-stream transport handed to handler tasks.
//!
//! [`H2Transport`] is the [`AsyncRead`] + [`AsyncWrite`] view of a single HTTP/2 stream. It is
//! carried on the emitted [`Conn`][crate::Conn] returned from [`H2Acceptor::next`], and the
//! runtime adapter spawns a handler task that consumes it. The transport never touches the
//! underlying TCP connection directly — all I/O coordinates through shared per-stream state
//! on the [`H2Connection`] driven by the acceptor task.
//!
//! During normal HTTP/2 operation neither impl is invoked on the production paths:
//! [`ReceivedBody`][crate::ReceivedBody] reads the request body via the transport's
//! `AsyncRead`, but through `ReceivedBody::handle_h2_data` — users generally don't reach for
//! the transport directly (same sharp edge h1 and h3 document). Response bytes flow through
//! [`H2Connection::submit_send`][submit_send] to the driver's send pump, which frames HEADERS
//! + DATA + trailing HEADERS onto the connection without ever touching this `AsyncWrite`.
//!
//! The real `AsyncRead` + `AsyncWrite` impls remain in place anyway because a future
//! extended-CONNECT upgrade ([RFC 8441] WebSocket-over-h2, [RFC 9220] WebTransport-over-h2)
//! keeps a single h2 stream open as a bidirectional byte channel after the response — at that
//! point the upgrade handler needs a real transport to talk over, and `H2Transport` is the
//! slot the `Conn.transport` already points at. See `memory/h2-planning.md` "Future-proofing:
//! extended CONNECT."
//!
//! [`H2Acceptor::next`]: super::H2Acceptor::next
//! [`H2Connection`]: super::H2Connection
//! [`BoxedTransport`]: crate::transport::BoxedTransport
//! [submit_send]: super::H2Connection::submit_send
//! [RFC 8441]: https://www.rfc-editor.org/rfc/rfc8441
//! [RFC 9220]: https://www.rfc-editor.org/rfc/rfc9220

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
    /// Create a transport for a stream that has just been opened by the acceptor.
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
        // No-op. The production send path routes bytes through
        // `H2Connection::submit_send` (response framing) and will eventually route through a
        // per-stream outbound-byte queue for extended-CONNECT upgrades — neither touches this
        // impl. A caller that borrows the transport and writes directly is on the same
        // "you're on your own" footing as the h1 and h3 transports; we accept silently rather
        // than stall the handler's progress.
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
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
    /// [`H2Acceptor::complete_and_remove_stream`][super::H2Acceptor]'s normal cleanup path.
    ///
    /// [`H2Acceptor`]: super::H2Acceptor
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

/// Send-side per-stream state used to hand a response from the conn task to the driver.
///
/// The conn task fills `submission` once via [`H2Connection::submit_send`][submit] and waits on
/// `completion_waker` for `completed` to flip. The driver picks up the submission on its next
/// `drive` tick, frames it (HEADERS, DATA, optional trailing HEADERS) into the connection's
/// outbound buffer as send-side flow control allows, and on completion stores the
/// `completion_result`, sets `completed = true`, and wakes the conn task.
///
/// The shape is general enough to absorb a future `outbound_bytes: VecDeque<u8>` queue for
/// extended-CONNECT (WebSocket / WebTransport over h2) upgrades — the upgrade handler's
/// `AsyncWrite` impl would push raw bytes into that queue alongside (or instead of) `body`.
///
/// [submit]: super::H2Connection::submit_send
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
}

/// What the conn task hands the driver for a single response. Body's trailers (if any) are
/// pulled by the driver via `Body::trailers()` after the body is fully drained — they are not
/// a separate field here.
#[derive(Debug)]
pub(super) struct Submission {
    pub(super) encoded_headers: Vec<u8>,
    pub(super) body: Option<Body>,
}
