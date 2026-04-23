//! Per-stream transport handed to handler tasks.
//!
//! [`H2Transport`] is the [`AsyncRead`] + [`AsyncWrite`] view of a single HTTP/2 stream. It is
//! returned from [`H2Acceptor::next`] and the runtime adapter spawns a handler task that consumes
//! it. The transport never touches the underlying TCP connection directly — all I/O coordinates
//! through shared per-stream state on the [`H2Connection`] driven by the acceptor task.
//!
//! The send side (`poll_write` / `poll_close`) is a no-op placeholder: phase 4's `Conn::send_h2`
//! will submit the whole response (headers + Body + trailers) to [`H2Connection`] and let the
//! driver schedule frames onto the shared transport. The `AsyncWrite` impl exists to satisfy
//! [`BoxedTransport`] bounds but is not exercised on the production send path. See
//! `memory/h2-planning.md` "Lay of the land" — design 1.5.
//!
//! [`H2Acceptor::next`]: super::H2Acceptor::next
//! [`H2Connection`]: super::H2Connection
//! [`BoxedTransport`]: crate::transport::BoxedTransport

use super::H2Connection;
use crate::{Body, Buffer};
use atomic_waker::AtomicWaker;
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    fmt, io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
};

/// A single HTTP/2 stream's transport handle.
///
/// Today (during phase-4 step 2) `H2Transport` still carries an [`Arc<StreamState>`] and a real
/// [`AsyncRead`] impl that drains the stream's recv ring — that code path will be replaced in
/// step 6 by `ReceivedBody` reading via `H2Connection::poll_read`, after which `H2Transport`
/// collapses to a unit struct with loud-fail `AsyncRead`/`AsyncWrite` stubs whose only purpose
/// is to satisfy [`BoxedTransport`][crate::transport::BoxedTransport]'s trait bounds at the
/// `Conn.transport` slot. Until then the type still needs the connection backref + stream id +
/// state Arc to implement `poll_read`.
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
            connection.outbound_waker.wake();
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
        // Placeholder. The production send path in phase 4 submits the whole response to
        // `H2Connection` via `submit_response` and bypasses this entirely; `AsyncWrite` only
        // exists here to satisfy `BoxedTransport` bounds. If a caller does invoke it, accept
        // the bytes silently rather than blackhole the handler's progress.
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
}

/// Send-side per-stream state used to hand a response from the conn task to the driver.
///
/// The conn task fills `submission` once via [`H2Connection::submit_send`][submit] and waits on
/// `completion_waker` for `completed` to flip. The driver picks up the submission on its next
/// `poll_next` tick, frames it (HEADERS, DATA, optional trailing HEADERS) into the connection's
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
