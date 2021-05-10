use crate::{Conn, Stopper};
use futures_lite::{AsyncRead, AsyncWrite};
use http_types::{headers::Headers, Extensions, Method};
use std::{
    fmt::{self, Debug, Formatter},
    io,
    pin::Pin,
    str,
    task::{Context, Poll},
};

/**
This open (pub fields) struct represents a http upgrade. It contains
all of the data available on a Conn, as well as owning the underlying
transport.

Important implementation note: When reading directly from the
transport, ensure that you read from `buffer` first if there are bytes
in it. Alternatively, read directly from the Upgrade, as that
AsyncRead implementation will drain the buffer first before reading
from the transport.
*/
pub struct Upgrade<Transport> {
    /// The http request headers
    pub request_headers: Headers,
    /// The request path
    pub path: String,
    /// The http request method
    pub method: Method,
    /// Any state that has been accumulated on the Conn before negotiating the upgrade
    pub state: Extensions,
    /// The underlying io (often a TcpStream or similar)
    pub transport: Transport,
    /// Any bytes that have been read from the underlying tcpstream
    /// already. It is your responsibility to process these bytes
    /// before reading directly from the transport.
    pub buffer: Option<Vec<u8>>,
    /// A [`Stopper`] which can and should be used to gracefully shut
    /// down any long running streams or futures associated with this
    /// upgrade
    pub stopper: Stopper,
}

impl<Transport> Upgrade<Transport> {
    /// read-only access to the request headers
    pub fn headers(&self) -> &Headers {
        &self.request_headers
    }

    /// the http request path
    pub fn path(&self) -> &str {
        &self.path
    }

    /// the http method
    pub fn method(&self) -> &Method {
        &self.method
    }

    /// any state that has been accumulated on the Conn before
    /// negotiating the upgrade.
    pub fn state(&self) -> &Extensions {
        &self.state
    }

    /// Modify the transport type of this upgrade. This is useful for
    /// boxing the transport in order to erase the type argument.
    pub fn map_transport<T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static>(
        self,
        f: impl Fn(Transport) -> T,
    ) -> Upgrade<T> {
        Upgrade {
            transport: f(self.transport),
            path: self.path,
            method: self.method,
            state: self.state,
            buffer: self.buffer,
            request_headers: self.request_headers,
            stopper: self.stopper,
        }
    }
}

impl<Transport> Debug for Upgrade<Transport> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(&format!("Upgrade<{}>", std::any::type_name::<Transport>()))
            .field("request_headers", &self.request_headers)
            .field("path", &self.path)
            .field("method", &self.method)
            .field(
                "buffer",
                &self.buffer.as_deref().map(String::from_utf8_lossy),
            )
            .finish()
    }
}

impl<Transport> From<Conn<Transport>> for Upgrade<Transport> {
    fn from(conn: Conn<Transport>) -> Self {
        let Conn {
            request_headers,
            path,
            method,
            state,
            transport,
            buffer,
            stopper,
            ..
        } = conn;

        Self {
            request_headers,
            path,
            method,
            state,
            transport,
            buffer,
            stopper,
        }
    }
}

impl<Transport: AsyncRead + Unpin> AsyncRead for Upgrade<Transport> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match self.buffer.take() {
            Some(mut buffer) if !buffer.is_empty() => {
                let len = buffer.len();
                if len > buf.len() {
                    log::trace!(
                        "have {} bytes of pending data but can only use {}",
                        len,
                        buf.len()
                    );
                    let remaining = buffer.split_off(buf.len());
                    buf.copy_from_slice(&buffer[..]);
                    self.buffer = Some(remaining);
                    Poll::Ready(Ok(buf.len()))
                } else {
                    log::trace!("have {} bytes of pending data, using all of it", len);
                    buf[..len].copy_from_slice(&buffer);
                    self.buffer = None;
                    match Pin::new(&mut self.transport).poll_read(cx, &mut buf[len..]) {
                        Poll::Ready(Ok(e)) => Poll::Ready(Ok(e + len)),
                        Poll::Pending => Poll::Ready(Ok(len)),
                        other => other,
                    }
                }
            }

            _ => Pin::new(&mut self.transport).poll_read(cx, buf),
        }
    }
}

impl<Transport: AsyncWrite + Unpin> AsyncWrite for Upgrade<Transport> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.transport).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.transport).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.transport).poll_close(cx)
    }
}
