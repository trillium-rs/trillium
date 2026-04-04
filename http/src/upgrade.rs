use crate::{
    Buffer, Conn, Headers, HttpContext, Method, TypeSet, Version, h3::H3Connection,
    received_body::read_buffered,
};
use fieldwork::Fieldwork;
use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    borrow::Cow,
    fmt::{self, Debug, Formatter},
    io,
    net::IpAddr,
    pin::Pin,
    str,
    sync::Arc,
    task::{self, Poll},
};
use trillium_macros::AsyncWrite;

/// This struct represents a http upgrade. It contains all of the data available on a Conn, as well
/// as owning the underlying transport.
///
/// **Important implementation note**: When reading directly from the transport, ensure that you
/// read from `buffer` first if there are bytes in it. Alternatively, read directly from the
/// Upgrade, as that [`AsyncRead`] implementation will drain the buffer first before reading from
/// the transport.
#[derive(AsyncWrite, Fieldwork)]
#[fieldwork(get, get_mut, set, with, take, into_field, rename_predicates)]
pub struct Upgrade<Transport> {
    /// The http request headers
    request_headers: Headers,

    /// The request path
    #[field(get = false)]
    path: Cow<'static, str>,

    /// The http request method
    #[field(copy)]
    method: Method,

    /// Any state that has been accumulated on the Conn before negotiating the upgrade
    state: TypeSet,

    /// The underlying io (often a `TcpStream` or similar)
    #[async_write]
    transport: Transport,

    /// Any bytes that have been read from the underlying transport already.
    ///
    /// It is your responsibility to process these bytes before reading directly from the
    /// transport.
    #[field(deref = "[u8]", into_field = false, set = false, with = false)]
    buffer: Buffer,

    /// The [`HttpContext`] shared for this server
    #[field(deref = false)]
    context: Arc<HttpContext>,

    /// the ip address of the connection, if available
    #[field(copy)]
    peer_ip: Option<IpAddr>,

    /// the :authority http/3 pseudo-header
    authority: Option<Cow<'static, str>>,

    /// the :scheme http/3 pseudo-header
    scheme: Option<Cow<'static, str>>,

    /// the HTTP/3 connection associated with this upgrade, if this was an HTTP/3 connection
    #[field(get(deref = false))]
    h3_connection: Option<Arc<H3Connection>>,

    /// the :protocol http/3 pseudo-header
    protocol: Option<Cow<'static, str>>,

    /// the http version
    #[field = "http_version"]
    version: Version,

    /// whether this connection was deemed secure by the handler stack
    secure: bool,
}

impl<Transport> Upgrade<Transport> {
    #[doc(hidden)]
    pub fn new(
        request_headers: Headers,
        path: impl Into<Cow<'static, str>>,
        method: Method,
        transport: Transport,
        buffer: Buffer,
        version: Version,
    ) -> Self {
        Self {
            request_headers,
            path: path.into(),
            method,
            transport,
            buffer,
            state: TypeSet::new(),
            context: Arc::default(),
            peer_ip: None,
            authority: None,
            scheme: None,
            h3_connection: None,
            protocol: None,
            secure: false,
            version,
        }
    }

    /// Take any buffered bytes
    pub fn take_buffer(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buffer).into()
    }

    #[doc(hidden)]
    pub fn buffer_and_transport_mut(&mut self) -> (&mut Buffer, &mut Transport) {
        (&mut self.buffer, &mut self.transport)
    }

    /// borrow the shared state [`TypeSet`] for this application
    pub fn shared_state(&self) -> &TypeSet {
        self.context.shared_state()
    }

    /// the http request path up to but excluding any query component
    pub fn path(&self) -> &str {
        match self.path.split_once('?') {
            Some((path, _)) => path,
            None => &self.path,
        }
    }

    /// retrieves the query component of the path
    pub fn querystring(&self) -> &str {
        self.path
            .split_once('?')
            .map(|(_, query)| query)
            .unwrap_or_default()
    }

    /// Modify the transport type of this upgrade.
    ///
    /// This is useful for boxing the transport in order to erase the type argument.
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
            context: self.context,
            peer_ip: self.peer_ip,
            authority: self.authority,
            scheme: self.scheme,
            h3_connection: self.h3_connection,
            protocol: self.protocol,
            version: self.version,
            secure: self.secure,
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
            .field("context", &self.context)
            .field("state", &self.state)
            .field("transport", &format_args!(".."))
            .field("peer_ip", &self.peer_ip)
            .field("authority", &self.authority)
            .field("scheme", &self.scheme)
            .field("h3_connection", &self.h3_connection)
            .field("protocol", &self.protocol)
            .field("version", &self.version)
            .field("secure", &self.secure)
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
            context,
            peer_ip,
            authority,
            scheme,
            h3_connection,
            protocol,
            version,
            secure,
            ..
        } = conn;

        Self {
            request_headers,
            path,
            method,
            state,
            transport,
            buffer,
            context,
            peer_ip,
            authority,
            scheme,
            h3_connection,
            protocol,
            version,
            secure,
        }
    }
}

impl<Transport: AsyncRead + Unpin> AsyncRead for Upgrade<Transport> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let Self {
            transport, buffer, ..
        } = &mut *self;
        read_buffered(buffer, transport, cx, buf)
    }
}
