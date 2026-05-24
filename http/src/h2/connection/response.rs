//! Client-role primitives for taking the response off a stream's recv side.
//!
//! [`ResponseHeaders`] awaits the peer's first HEADERS frame and yields the decoded
//! [`FieldSection`]. [`H2Connection::take_trailers`][super::H2Connection::take_trailers] pops
//! trailers stashed by the driver after the body's `END_STREAM`.

use super::H2Connection;
use crate::Headers;
#[cfg(feature = "unstable")]
use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
};

/// Future returned by [`H2Connection::response_headers`].
///
/// Awaits the peer's first HEADERS frame on a client-initiated stream and yields the decoded
/// [`FieldSection`][crate::headers::hpack::FieldSection]. See
/// [`H2Connection::response_headers`] for error semantics.
#[cfg(feature = "unstable")]
#[must_use = "futures do nothing unless awaited"]
#[derive(Debug)]
pub struct ResponseHeaders<'a> {
    pub(super) connection: &'a H2Connection,
    pub(super) stream_id: u32,
}

#[cfg(feature = "unstable")]
impl Future for ResponseHeaders<'_> {
    type Output = io::Result<crate::headers::hpack::FieldSection<'static>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Some(state) = self.connection.streams_lock().get(&self.stream_id).cloned() else {
            return Poll::Ready(Err(io::ErrorKind::NotConnected.into()));
        };
        let try_take = || {
            state
                .recv
                .response_headers
                .lock()
                .expect("response_headers mutex poisoned")
                .take()
        };
        if let Some(fs) = try_take() {
            return Poll::Ready(Ok(fs));
        }
        if state.lifecycle_lock().recv_closed() {
            return Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()));
        }
        state.recv.response_headers_waker.register(cx.waker());
        // Re-check after registering so we don't miss a wake fired between the load above
        // and the registration.
        if let Some(fs) = try_take() {
            return Poll::Ready(Ok(fs));
        }
        if state.lifecycle_lock().recv_closed() {
            return Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()));
        }
        Poll::Pending
    }
}

impl H2Connection {
    /// Client-role: await the response HEADERS field section for a stream.
    ///
    /// Resolves to the decoded [`FieldSection`] (including h2 pseudo-headers like `:status`)
    /// once the driver receives and stashes the peer's first HEADERS frame on this stream.
    /// Callers typically decompose it via [`FieldSection::into_parts`] to populate
    /// user-facing `Headers` + status.
    ///
    /// Single-shot: the `FieldSection` is moved out on a successful poll, so subsequent calls
    /// for the same stream id will surface `ConnectionAborted` rather than re-deliver the
    /// headers.
    ///
    /// Errors:
    /// - `NotConnected` — stream id is no longer tracked by the driver.
    /// - `ConnectionAborted` — recv side reached eof without HEADERS arriving (peer reset the
    ///   stream, sent GOAWAY, or otherwise tore the connection down).
    ///
    /// [`FieldSection`]: crate::headers::hpack::FieldSection
    /// [`FieldSection::into_parts`]: crate::headers::hpack::FieldSection::into_parts
    #[cfg(feature = "unstable")]
    pub fn response_headers(&self, stream_id: u32) -> ResponseHeaders<'_> {
        ResponseHeaders {
            connection: self,
            stream_id,
        }
    }

    /// Remove and return trailers stashed on the stream's recv state. Returns `None` if
    /// the stream is gone (already closed) or no trailers were received.
    pub(crate) fn take_trailers(&self, stream_id: u32) -> Option<Headers> {
        let stream = self.streams_lock().get(&stream_id).cloned()?;
        stream
            .recv
            .trailers
            .lock()
            .expect("recv trailers mutex poisoned")
            .take()
    }
}
