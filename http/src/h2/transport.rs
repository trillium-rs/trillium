//! Per-stream transport handed to handler tasks.
//!
//! [`H2Transport`] is the [`AsyncRead`] + [`AsyncWrite`] view of a single HTTP/2 stream. It is
//! returned from [`H2Acceptor::next`] and the runtime adapter spawns a handler task that consumes
//! it. The transport never touches the underlying TCP connection directly — all I/O coordinates
//! through shared per-stream state on the [`H2Connection`] driven by the acceptor task.
//!
//! Phase 3 (in progress): the type is wired up but `poll_read` and `poll_write` are stubs.
//! Subsequent commits add DATA-frame routing on the read side and the send buffer + driver wake
//! on the write side.
//!
//! [`H2Acceptor::next`]: super::H2Acceptor::next
//! [`H2Connection`]: super::H2Connection

use super::H2Connection;
use crate::headers::hpack::FieldSection;
use atomic_waker::AtomicWaker;
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    collections::VecDeque,
    fmt, io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
};

/// A single HTTP/2 stream, presented as an [`AsyncRead`] + [`AsyncWrite`] for handler-side code.
///
/// Each stream that opens on a connection produces one `H2Transport` returned from
/// [`H2Acceptor::next`][crate::h2::H2Acceptor::next]. The runtime adapter spawns a task per
/// transport, builds a [`Conn`][crate::Conn] from it, and runs the user's handler. The transport
/// holds an [`Arc`] to the shared [`H2Connection`] so that frame I/O continues to flow through the
/// single driver task even as the handler reads body bytes and writes its response.
///
/// The decoded request [`FieldSection`] arrives attached to the transport — handler-side
/// `Conn::new_h2` takes it via [`Self::take_request_headers`] before the user handler runs.
pub struct H2Transport {
    connection: Arc<H2Connection>,
    stream_id: u32,
    request_headers: Option<FieldSection<'static>>,
    state: Arc<StreamState>,
}

impl fmt::Debug for H2Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("H2Transport")
            .field("stream_id", &self.stream_id)
            .field("has_request_headers", &self.request_headers.is_some())
            .finish_non_exhaustive()
    }
}

impl H2Transport {
    /// Create a transport for a stream that has just been opened by the acceptor.
    pub(super) fn new(
        connection: Arc<H2Connection>,
        stream_id: u32,
        request_headers: FieldSection<'static>,
        state: Arc<StreamState>,
    ) -> Self {
        Self {
            connection,
            stream_id,
            request_headers: Some(request_headers),
            state,
        }
    }

    /// The stream identifier this transport is bound to.
    pub fn stream_id(&self) -> u32 {
        self.stream_id
    }

    /// The shared [`H2Connection`] backing this stream. Handler-side code calls into this for
    /// per-stream operations that don't fit into [`AsyncRead`] / [`AsyncWrite`] — currently, just
    /// trailers retrieval (lands in a follow-up phase).
    pub fn connection(&self) -> &Arc<H2Connection> {
        &self.connection
    }

    /// Take the decoded request headers. Returns `None` after the first call — the handler-side
    /// `Conn::new_h2` consumes them exactly once.
    pub fn take_request_headers(&mut self) -> Option<FieldSection<'static>> {
        self.request_headers.take()
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

        let recv_state = &self.state.recv;
        let mut recv = recv_state.buf.lock().expect("recv buf mutex poisoned");

        // Drain bytes from the front of the queue into `out`. Each entry in `recv_buf` is one
        // DATA frame's payload; we consume head entries fully and leave a partially-read entry
        // at the front for the next call.
        let mut written = 0;
        while written < out.len() {
            let Some(front) = recv.front_mut() else { break };
            let take = (out.len() - written).min(front.len());
            out[written..written + take].copy_from_slice(&front[..take]);
            written += take;
            if take == front.len() {
                recv.pop_front();
            } else {
                front.drain(..take);
            }
        }

        if written > 0 {
            return Poll::Ready(Ok(written));
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
        // Phase 3 stub. Send buffering + driver wake land in a follow-up commit; for now silently
        // accept the bytes and discard so a handler that calls write doesn't error.
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
///
/// Phase 3 in progress: receive side wired up (DATA frame routing + EOF + waker). Send ring +
/// waker and flow control windows land in subsequent commits.
#[derive(Debug, Default)]
pub(super) struct StreamState {
    /// Recv side: inbound DATA payloads, EOF flag, handler waker.
    pub(super) recv: RecvState,
}

/// Receive-side per-stream state.
#[derive(Debug, Default)]
pub(super) struct RecvState {
    /// Inbound DATA payloads awaiting handler read. Each entry is one DATA frame's payload —
    /// the driver pushes whole payloads, [`H2Transport::poll_read`] drains them entry-by-entry,
    /// splitting head entries when a partial copy is needed.
    pub(super) buf: Mutex<VecDeque<Vec<u8>>>,

    /// `true` once `END_STREAM` has been observed for this stream's recv side. Set by the
    /// driver under the same `buf` lock used for pushes; checked by `poll_read` while
    /// holding that lock to decide between EOF and Pending.
    pub(super) eof: AtomicBool,

    /// Handler-task waker, fired by the driver after pushing DATA into `buf` or after
    /// setting `eof`. Single-waiter: only one task ever polls a given `H2Transport`.
    pub(super) waker: AtomicWaker,
}
