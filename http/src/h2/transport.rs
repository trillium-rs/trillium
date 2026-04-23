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
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    fmt,
    io,
    pin::Pin,
    sync::Arc,
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
        _cx: &mut Context<'_>,
        _out: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        // Phase 3 stub. DATA-frame routing lands in a follow-up commit; for now signal EOF so
        // any handler that tries to read returns immediately rather than hanging.
        Poll::Ready(Ok(0))
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
/// Phase 3 minimum: empty. Receive ring + waker, send ring + waker, flow control windows, and
/// trailers slot land in subsequent commits.
#[derive(Debug, Default)]
pub(super) struct StreamState {}
