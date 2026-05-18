use crate::{
    Buffer, Conn, Headers, HttpContext, Method, ProtocolSession, Status, TypeSet, Version,
    h2::H2Connection, h3::H3Connection, received_body::read_buffered,
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
    time::Instant,
};
use trillium_macros::AsyncWrite;

/// An HTTP upgrade — owns the underlying transport along with all the data from the
/// originating [`Conn`].
///
/// **Reading the transport directly**: drain `buffer` first if it has bytes in it. Reading
/// via the [`AsyncRead`] impl on `Upgrade` handles this automatically.
#[derive(AsyncWrite, Fieldwork)]
#[fieldwork(get, get_mut, set, with, take, into_field, rename_predicates)]
pub struct Upgrade<Transport> {
    /// The http request headers
    request_headers: Headers,

    /// The http response headers as set on the underlying [`Conn`] before the upgrade was
    /// negotiated. These have already been sent to the peer; preserved here for inspection.
    response_headers: Headers,

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

    /// the wall-clock time at which the underlying [`Conn`] was constructed. Useful for
    /// instrumentation that wants elapsed time across the upgrade transition.
    #[field(copy)]
    start_time: Instant,

    /// the :authority http/3 pseudo-header
    authority: Option<Cow<'static, str>>,

    /// the :scheme http/3 pseudo-header
    scheme: Option<Cow<'static, str>>,

    /// the [`ProtocolSession`] for this upgrade — bundles the per-protocol session state
    /// (h2/h3 connection driver and stream id) that was attached to the originating Conn.
    /// `Http1` for upgrades from h1 / synthetic conns.
    #[field = false]
    protocol_session: ProtocolSession,

    /// the :protocol http/3 pseudo-header
    protocol: Option<Cow<'static, str>>,

    /// the http version
    #[field = "http_version"]
    version: Version,

    /// the http response status set on the underlying [`Conn`] at the time the upgrade was
    /// negotiated (typically `101 Switching Protocols` or `200 OK` for CONNECT). `None` if no
    /// status was set explicitly.
    #[field(copy)]
    status: Option<Status>,

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
            response_headers: Headers::new(),
            path: path.into(),
            method,
            transport,
            buffer,
            state: TypeSet::new(),
            context: Arc::default(),
            peer_ip: None,
            start_time: Instant::now(),
            authority: None,
            scheme: None,
            protocol_session: ProtocolSession::Http1,
            protocol: None,
            secure: false,
            version,
            status: None,
        }
    }

    /// the [`H2Connection`] driver for this upgrade, if it originated from an HTTP/2 stream
    pub fn h2_connection(&self) -> Option<&Arc<H2Connection>> {
        self.protocol_session.h2_connection()
    }

    /// the h2 stream id for this upgrade, if it originated from an HTTP/2 stream
    pub fn h2_stream_id(&self) -> Option<u32> {
        self.protocol_session.h2_stream_id()
    }

    /// the [`H3Connection`] driver for this upgrade, if it originated from an HTTP/3 stream
    pub fn h3_connection(&self) -> Option<&Arc<H3Connection>> {
        self.protocol_session.h3_connection()
    }

    /// the h3 stream id for this upgrade, if it originated from an HTTP/3 stream
    pub fn h3_stream_id(&self) -> Option<u64> {
        self.protocol_session.h3_stream_id()
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
        // Manual respread: rustc treats `Upgrade<Transport>` and `Upgrade<T>` as disjoint
        // and rejects `..self` without the unstable `type_changing_struct_update` feature.
        // If a new field is added to `Upgrade`, update this respread, `Conn::map_transport`
        // (`conn.rs`), and `From<Conn> for Upgrade` below — they share this drift hazard.
        Upgrade {
            transport: f(self.transport),
            path: self.path,
            method: self.method,
            state: self.state,
            buffer: self.buffer,
            request_headers: self.request_headers,
            response_headers: self.response_headers,
            context: self.context,
            peer_ip: self.peer_ip,
            start_time: self.start_time,
            authority: self.authority,
            scheme: self.scheme,
            protocol_session: self.protocol_session,
            protocol: self.protocol,
            version: self.version,
            status: self.status,
            secure: self.secure,
        }
    }
}

impl<Transport> Debug for Upgrade<Transport> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(&format!("Upgrade<{}>", std::any::type_name::<Transport>()))
            .field("request_headers", &self.request_headers)
            .field("response_headers", &self.response_headers)
            .field("path", &self.path)
            .field("method", &self.method)
            .field("buffer", &self.buffer)
            .field("context", &self.context)
            .field("state", &self.state)
            .field("transport", &format_args!(".."))
            .field("peer_ip", &self.peer_ip)
            .field("start_time", &self.start_time)
            .field("authority", &self.authority)
            .field("scheme", &self.scheme)
            .field("protocol_session", &self.protocol_session)
            .field("protocol", &self.protocol)
            .field("version", &self.version)
            .field("status", &self.status)
            .field("secure", &self.secure)
            .finish()
    }
}

impl<Transport> From<Conn<Transport>> for Upgrade<Transport> {
    fn from(conn: Conn<Transport>) -> Self {
        // Exhaustive destructure (no `..` rest pattern) so that adding a new field to
        // `Conn` is a compile error here, forcing a deliberate carry-vs-drop decision
        // for the upgrade transition. The discarded fields below are response-body /
        // request-body / instrumentation state that is meaningless once the conn has
        // crossed into the upgrade phase. This shares a drift hazard with
        // `Conn::map_transport` (`conn.rs`) and `Upgrade::map_transport` above.
        let Conn {
            request_headers,
            response_headers,
            path,
            method,
            state,
            transport,
            buffer,
            context,
            peer_ip,
            start_time,
            authority,
            scheme,
            protocol_session,
            protocol,
            version,
            status,
            secure,
            // Deliberately dropped — response-body / request-body lifecycle state with
            // no role on the upgraded transport.
            response_body: _,
            request_body_state: _,
            after_send: _,
            request_trailers: _,
        } = conn;

        Self {
            request_headers,
            response_headers,
            path,
            method,
            state,
            transport,
            buffer,
            context,
            peer_ip,
            start_time,
            authority,
            scheme,
            protocol_session,
            protocol,
            version,
            status,
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
