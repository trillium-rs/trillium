//! Serving HTTP/1.x on a transport.
//!
//! [`H1Connection`] runs a handler for each request on one HTTP/1.x connection, and reports how the
//! connection ended as an [`H1ConnectionResult`].

use crate::{
    Conn, Error, HttpContext, Upgrade,
    conn::{ConnInner, H1Status, HeadError},
    graceful_close::GracefulClose,
};
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    fmt::{self, Debug, Formatter},
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

/// An HTTP/1.x connection on one transport.
///
/// Configure it with the `with_*` methods, then call [`run`][Self::run] with a handler to serve
/// requests on it.
pub struct H1Connection<Transport> {
    context: Arc<HttpContext>,
    transport: Transport,
    initial_bytes: Option<Vec<u8>>,
}

impl<Transport> Debug for H1Connection<Transport> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("H1Connection")
            .field("initial_bytes", &self.initial_bytes.as_ref().map(Vec::len))
            .finish_non_exhaustive()
    }
}

impl<Transport> H1Connection<Transport> {
    /// An HTTP/1.x connection on `transport`, sharing `context` with the rest of the server.
    pub fn new(context: Arc<HttpContext>, transport: Transport) -> Self {
        Self {
            context,
            transport,
            initial_bytes: None,
        }
    }

    /// Bytes already read from the transport, such as those read while checking for an HTTP/2
    /// connection preface. They're parsed as the start of the first request, before anything else
    /// is read.
    #[must_use]
    pub fn with_initial_bytes(mut self, initial_bytes: Vec<u8>) -> Self {
        self.initial_bytes = Some(initial_bytes);
        self
    }
}

impl<Transport> H1Connection<Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    /// Serve requests on this connection, calling `handler` once for each, until the connection
    /// closes, upgrades, or fails.
    pub async fn run<Handler, Fut>(self, mut handler: Handler) -> H1ConnectionResult<Transport>
    where
        Handler: FnMut(Conn<Transport>) -> Fut,
        Fut: Future<Output = Conn<Transport>>,
    {
        let Self {
            context,
            transport,
            initial_bytes,
        } = self;
        let initial_bytes = initial_bytes
            .unwrap_or_else(|| Vec::with_capacity(context.config.request_buffer_initial_len));
        let _guard = context.swansong.guard();

        let mut status = match Conn::new_h1(context, transport, initial_bytes).await {
            Ok(conn) => Ok(H1Status::Conn(conn)),
            Err(HeadError::BadRequest(bad)) => bad.send().await,
            Err(HeadError::Fatal(error)) => Err(error),
        };

        loop {
            status = match status {
                Ok(H1Status::Conn(conn)) => handler(conn).await.send().await,
                Ok(H1Status::Upgrade(upgrade)) => return H1ConnectionResult::Upgrade(upgrade),
                Ok(H1Status::Close(conn)) => return H1ConnectionResult::Close(H1Close::new(conn)),
                Ok(H1Status::Ended(conn)) => {
                    return H1ConnectionResult::Close(H1Close::finished(conn));
                }
                Err(error) => return H1ConnectionResult::Error(error),
            };
        }
    }
}

/// How an HTTP/1.x connection ended.
#[derive(Debug)]
#[must_use]
#[non_exhaustive]
pub enum H1ConnectionResult<Transport> {
    /// The connection negotiated an upgrade, which now owns the transport.
    Upgrade(Upgrade<Transport>),

    /// The connection is finished. Await the [`H1Close`] to close it gracefully, or drop it to
    /// hang up immediately.
    Close(H1Close<Transport>),

    /// The connection ended with an error, such as a transport failure or a client that
    /// disconnected partway through a request.
    Error(Error),
}

impl<Transport> H1ConnectionResult<Transport> {
    /// The `HttpContext::run` shape: an upgrade if one was negotiated, hanging up on a close.
    pub(crate) fn into_upgrade(self) -> crate::Result<Option<Upgrade<Transport>>> {
        match self {
            Self::Upgrade(upgrade) => Ok(Some(upgrade)),
            Self::Close(_) => Ok(None),
            Self::Error(error) => Err(error),
        }
    }
}

/// The close of a finished HTTP/1.x connection.
///
/// Awaiting it shuts down our sending side of the transport, then reads and discards whatever the
/// client still sends until it closes its side. If the transport is dropped while the client is
/// still sending, the operating system answers with a TCP reset, which can destroy the end of a
/// response the client hasn't read yet. A client that never closes keeps this pending indefinitely,
/// so await it under a timeout. Dropping it closes the connection immediately.
///
/// When the client has already closed the connection, it completes immediately.
pub struct H1Close<Transport>(GracefulClose<Transport>);

impl<Transport> Debug for H1Close<Transport> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("H1Close").finish_non_exhaustive()
    }
}

impl<Transport> H1Close<Transport> {
    fn new(conn: Conn<Transport>) -> Self {
        let Conn { inner, transport } = conn;
        let ConnInner {
            buffer, context, ..
        } = *inner;
        Self(GracefulClose::new(
            transport,
            buffer.into(),
            context.config.copy_loops_per_yield,
        ))
    }

    fn finished(conn: Conn<Transport>) -> Self {
        Self(GracefulClose::finished(conn.transport))
    }
}

impl<Transport> Future for H1Close<Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        Pin::new(&mut self.get_mut().0).poll(cx)
    }
}
