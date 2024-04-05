use crate::{received_body::read_buffered, Buffer, Conn, Headers, Method, StateSet, Stopper};
use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    fmt::{self, Debug, Formatter},
    io,
    net::IpAddr,
    pin::Pin,
    str,
    task::{Context, Poll},
};
use trillium_macros::AsyncWrite;

/**
This open (pub fields) struct represents a http upgrade. It contains
all of the data available on a Conn, as well as owning the underlying
transport.

Important implementation note: When reading directly from the
transport, ensure that you read from `buffer` first if there are bytes
in it. Alternatively, read directly from the Upgrade, as that
[`AsyncRead`] implementation will drain the buffer first before
reading from the transport.
*/
#[derive(AsyncWrite)]
#[non_exhaustive]
pub struct Upgrade<Transport> {
    /// The http request headers
    pub request_headers: Headers,
    /// The request path
    pub path: String,
    /// The http request method
    pub method: Method,
    /// Any state that has been accumulated on the Conn before negotiating the upgrade
    pub state: StateSet,
    /// The underlying io (often a `TcpStream` or similar)
    #[async_write]
    pub transport: Transport,
    /// Any bytes that have been read from the underlying tcpstream
    /// already. It is your responsibility to process these bytes
    /// before reading directly from the transport.
    pub buffer: Buffer,
    /// A [`Stopper`] which can and should be used to gracefully shut
    /// down any long running streams or futures associated with this
    /// upgrade
    pub stopper: Stopper,
    /// the ip address of the connection, if available
    pub peer_ip: Option<IpAddr>,
}

impl<Transport> Upgrade<Transport> {
    #[doc(hidden)]
    pub fn new(
        request_headers: Headers,
        path: String,
        method: Method,
        transport: Transport,
        buffer: Buffer,
    ) -> Self {
        Self {
            request_headers,
            path,
            method,
            transport,
            buffer,
            state: StateSet::new(),
            stopper: Stopper::new(),
            peer_ip: None,
        }
    }

    /// read-only access to the request headers
    pub fn headers(&self) -> &Headers {
        &self.request_headers
    }

    /// the http request path up to but excluding any query component
    pub fn path(&self) -> &str {
        match self.path.split_once('?') {
            Some((path, _)) => path,
            None => &self.path,
        }
    }

    /**
        retrieves the query component of the path
    */
    pub fn querystring(&self) -> &str {
        self.path
            .split_once('?')
            .map(|(_, query)| query)
            .unwrap_or_default()
    }

    /// the http method
    pub fn method(&self) -> &Method {
        &self.method
    }

    /// any state that has been accumulated on the Conn before
    /// negotiating the upgrade.
    pub fn state(&self) -> &StateSet {
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
            peer_ip: self.peer_ip,
        }
    }
}

impl<Transport> Debug for Upgrade<Transport> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(&format!("Upgrade<{}>", std::any::type_name::<Transport>()))
            .field("request_headers", &self.request_headers)
            .field("path", &self.path)
            .field("method", &self.method)
            .field("buffer", &self.buffer)
            .field("stopper", &self.stopper)
            .field("state", &self.state)
            .field("transport", &"..")
            .field("peer_ip", &self.peer_ip)
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
            peer_ip,
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
            peer_ip,
        }
    }
}

impl<Transport: AsyncRead + Unpin> AsyncRead for Upgrade<Transport> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let Self {
            transport, buffer, ..
        } = &mut *self;
        read_buffered(buffer, transport, cx, buf)
    }
}
