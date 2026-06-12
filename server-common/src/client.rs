use crate::{Runtime, RuntimeTrait, Transport, UdpTransport, Url};
use smallvec::SmallVec;
use std::{
    any::Any,
    borrow::Cow,
    fmt::{self, Debug, Formatter},
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
};

/// Everything a [`Connector`] needs to open one connection: where to dial, whether to wrap it in
/// TLS, and any per-connection ALPN.
///
/// Construct with [`new_with_host`](Destination::new_with_host) — resolve a domain, or dial
/// pre-resolved addresses while validating against the domain — or
/// [`new_with_socket_addrs`](Destination::new_with_socket_addrs) — dial pre-resolved addresses as a
/// bare IP, with no SNI. There is deliberately no `Default`: a destination with neither a host nor
/// any address is unconnectable, so each constructor fills at least one dial source.
#[derive(Debug, Clone)]
pub struct Destination {
    secure: bool,
    host: Option<String>,
    port: u16,
    addrs: SmallVec<[SocketAddr; 4]>,
    alpn: SmallVec<[Cow<'static, [u8]>; 4]>,
}

impl Destination {
    /// A destination identified by host name and port.
    ///
    /// With no [`addrs`](Destination::with_addrs) added, the connector resolves `host` itself.
    /// Adding pre-resolved addresses dials those instead while still validating the certificate
    /// against `host`.
    pub fn new_with_host(secure: bool, host: impl Into<String>, port: u16) -> Self {
        Self {
            secure,
            host: Some(host.into()),
            port,
            addrs: SmallVec::new(),
            alpn: SmallVec::new(),
        }
    }

    /// A destination identified only by pre-resolved socket addresses: a bare-IP connection with no
    /// SNI, where TLS validates against the address actually dialed. The port is taken from the
    /// first address.
    pub fn new_with_socket_addrs(
        secure: bool,
        addrs: impl IntoIterator<Item = SocketAddr>,
    ) -> Self {
        let addrs = addrs.into_iter().collect::<SmallVec<[SocketAddr; 4]>>();
        let port = addrs.first().map_or(0, SocketAddr::port);
        Self {
            secure,
            host: None,
            port,
            addrs,
            alpn: SmallVec::new(),
        }
    }

    /// Build a destination from a URL: maps `http`/`https` to plaintext/TLS and extracts the host
    /// and port. An IP-literal host becomes a
    /// [`new_with_socket_addrs`](Destination::new_with_socket_addrs) destination, so it is never
    /// sent to a resolver; a domain becomes a [`new_with_host`](Destination::new_with_host)
    /// destination.
    ///
    /// # Errors
    ///
    /// Returns an error if the scheme is neither `http` nor `https`, or the URL has no host or
    /// port.
    pub fn from_url(url: &Url) -> io::Result<Self> {
        let secure = match url.scheme() {
            "http" => false,
            "https" => true,
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown scheme {other}"),
                ));
            }
        };
        let port = url.port_or_known_default().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, format!("{url} missing port"))
        })?;
        match url.host() {
            Some(url::Host::Domain(domain)) => Ok(Self::new_with_host(secure, domain, port)),
            Some(url::Host::Ipv4(ip)) => Ok(Self::new_with_socket_addrs(
                secure,
                [SocketAddr::from((ip, port))],
            )),
            Some(url::Host::Ipv6(ip)) => Ok(Self::new_with_socket_addrs(
                secure,
                [SocketAddr::from((ip, port))],
            )),
            None => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{url} missing host"),
            )),
        }
    }

    /// Reconstruct a URL for this destination's origin, used by the default
    /// [`connect_to`](Connector::connect_to) implementation to fall back to
    /// [`connect`](Connector::connect). Pre-resolved addresses and ALPN are not represented.
    ///
    /// # Errors
    ///
    /// Returns an error if the destination has neither a host nor any address, or the result does
    /// not parse as a URL.
    pub fn to_url(&self) -> io::Result<Url> {
        let scheme = if self.secure { "https" } else { "http" };
        let authority = match &self.host {
            Some(host) => format!("{host}:{}", self.port),
            None => self
                .addrs
                .first()
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "destination has neither host nor addresses",
                    )
                })?
                .to_string(),
        };
        Url::parse(&format!("{scheme}://{authority}"))
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
    }

    /// Whether this destination should be reached over TLS.
    #[must_use]
    pub fn secure(&self) -> bool {
        self.secure
    }

    /// The host name used for resolution and certificate validation, or `None` for a bare-IP
    /// destination.
    #[must_use]
    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    /// The origin port. Pre-resolved [`addrs`](Destination::addrs) carry their own ports; this is
    /// the port used when resolving the host.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The pre-resolved addresses to dial, or an empty slice to resolve the host.
    #[must_use]
    pub fn addrs(&self) -> &[SocketAddr] {
        &self.addrs
    }

    /// The ALPN protocols to advertise for this connection, or an empty slice to use the
    /// connector's configured default.
    #[must_use]
    pub fn alpn(&self) -> &[Cow<'static, [u8]>] {
        &self.alpn
    }

    /// Set the pre-resolved addresses to dial, replacing any already present.
    #[must_use]
    pub fn with_addrs(mut self, addrs: impl IntoIterator<Item = SocketAddr>) -> Self {
        self.set_addrs(addrs);
        self
    }

    /// Set the pre-resolved addresses to dial, replacing any already present.
    pub fn set_addrs(&mut self, addrs: impl IntoIterator<Item = SocketAddr>) -> &mut Self {
        self.addrs = addrs.into_iter().collect();
        self
    }

    /// Set the ALPN protocols to advertise, replacing any already present.
    #[must_use]
    pub fn with_alpn(mut self, alpn: impl IntoIterator<Item = Cow<'static, [u8]>>) -> Self {
        self.set_alpn(alpn);
        self
    }

    /// Set the ALPN protocols to advertise, replacing any already present.
    pub fn set_alpn(&mut self, alpn: impl IntoIterator<Item = Cow<'static, [u8]>>) -> &mut Self {
        self.alpn = alpn.into_iter().collect();
        self
    }

    /// Return a copy of this destination with the `secure` flag overridden.
    #[must_use]
    pub fn with_secure(mut self, secure: bool) -> Self {
        self.secure = secure;
        self
    }
}

/// Interface for runtime and tls adapters for the trillium client
///
/// See
/// [`trillium_client`](https://docs.trillium.rs/trillium_client) for more
/// information on usage.
pub trait Connector: Send + Sync + 'static {
    /// the [`Transport`] that [`connect`](Connector::connect) returns
    type Transport: Transport;

    /// The [`RuntimeTrait`] for this Connector
    type Runtime: RuntimeTrait;

    /// The async UDP socket type for this connector. Used by QUIC adapters
    /// for HTTP/3 support. Connectors that do not support UDP should set
    /// this to `()`.
    type Udp: UdpTransport;

    /// Initiate a connection to the provided url
    fn connect(&self, url: &Url) -> impl Future<Output = io::Result<Self::Transport>> + Send;

    /// Open a connection to `destination`: dialing its pre-resolved addresses if present, otherwise
    /// resolving its host, and advertising any per-connection ALPN it carries.
    ///
    /// A domain destination keeps its host as the certificate identity (SNI) regardless of the
    /// addresses dialed, so pre-resolved addresses may come from any resolver (e.g. a DNS cache)
    /// without affecting certificate validation.
    ///
    /// The default implementation reconstructs a URL via [`Destination::to_url`] and calls
    /// [`connect`](Connector::connect), which ignores pre-resolved addresses and per-connection
    /// ALPN; connectors that honor those override this method.
    ///
    /// `destination` is taken by value so connectors can adjust it (e.g. clearing `secure` before
    /// delegating the TCP dial to an inner connector) without copying. A caller that needs to
    /// retain it should clone before calling.
    fn connect_to(
        &self,
        destination: Destination,
    ) -> impl Future<Output = io::Result<Self::Transport>> + Send {
        async move { self.connect(&destination.to_url()?).await }
    }

    /// Returns an object-safe [`ArcedConnector`]. Do not implement this.
    fn arced(self) -> ArcedConnector
    where
        Self: Sized,
    {
        ArcedConnector(Arc::new(self))
    }

    /// Perform a DNS lookup for a given host-and-port
    fn resolve(
        &self,
        host: &str,
        port: u16,
    ) -> impl Future<Output = io::Result<Vec<SocketAddr>>> + Send;

    /// Returns the runtime
    fn runtime(&self) -> Self::Runtime;
}

/// An Arced and type-erased [`Connector`]
#[derive(Clone)]
pub struct ArcedConnector(Arc<dyn ObjectSafeConnector>);

impl Debug for ArcedConnector {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ArcedConnector").finish()
    }
}

impl ArcedConnector {
    /// Constructs a new `ArcedConnector`
    #[must_use]
    pub fn new(connector: impl Connector) -> Self {
        connector.arced()
    }

    /// Determine if this `ArcedConnector` is the specified type
    pub fn is<T: Any + 'static>(&self) -> bool {
        self.as_any().is::<T>()
    }

    /// Attempt to borrow this `ArcedConnector` as the provided type, returning None if it does not
    /// contain the type
    pub fn downcast_ref<T: Any + 'static>(&self) -> Option<&T> {
        self.0.as_any().downcast_ref()
    }

    /// Attempt to mutably borrow this `ArcedConnector` as the provided type, returning None if it
    /// does not contain the type or if there are multiple outstanding clones of this arc
    pub fn downcast_mut<T: Any + 'static>(&mut self) -> Option<&mut T> {
        Arc::get_mut(&mut self.0)?.as_mut_any().downcast_mut()
    }

    /// Returns an object-safe [`Runtime`]
    pub fn runtime(&self) -> Runtime {
        self.0.runtime()
    }
}

// clippy thinks this is better ¯\(ツ)/¯
type ConnectResult<'fut> =
    Pin<Box<dyn Future<Output = io::Result<Box<dyn Transport>>> + Send + 'fut>>;

trait ObjectSafeConnector: Send + Sync + 'static {
    #[must_use]
    fn connect<'connector, 'url, 'fut>(&'connector self, url: &'url Url) -> ConnectResult<'fut>
    where
        'connector: 'fut,
        'url: 'fut,
        Self: 'fut;
    fn as_any(&self) -> &dyn Any;
    fn as_mut_any(&mut self) -> &mut dyn Any;
    fn runtime(&self) -> Runtime;

    fn resolve<'connector, 'host, 'fut>(
        &'connector self,
        host: &'host str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'fut>>
    where
        'connector: 'fut,
        'host: 'fut,
        Self: 'fut;

    #[must_use]
    fn connect_to<'connector, 'fut>(
        &'connector self,
        destination: Destination,
    ) -> ConnectResult<'fut>
    where
        'connector: 'fut,
        Self: 'fut;
}

impl<T: Connector> ObjectSafeConnector for T {
    fn connect<'connector, 'url, 'fut>(
        &'connector self,
        url: &'url Url,
    ) -> Pin<Box<dyn Future<Output = io::Result<Box<dyn Transport>>> + Send + 'fut>>
    where
        'connector: 'fut,
        'url: 'fut,
        Self: 'fut,
    {
        Box::pin(async move {
            Connector::connect(self, url)
                .await
                .map(|t| Box::new(t) as Box<dyn Transport>)
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_mut_any(&mut self) -> &mut dyn Any {
        self
    }

    fn runtime(&self) -> Runtime {
        Connector::runtime(self).into()
    }

    fn resolve<'connector, 'host, 'fut>(
        &'connector self,
        host: &'host str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'fut>>
    where
        'connector: 'fut,
        'host: 'fut,
        Self: 'fut,
    {
        Box::pin(async move { Connector::resolve(self, host, port).await })
    }

    fn connect_to<'connector, 'fut>(
        &'connector self,
        destination: Destination,
    ) -> ConnectResult<'fut>
    where
        'connector: 'fut,
        Self: 'fut,
    {
        Box::pin(async move {
            Connector::connect_to(self, destination)
                .await
                .map(|t| Box::new(t) as Box<dyn Transport>)
        })
    }
}

impl Connector for ArcedConnector {
    type Runtime = Runtime;
    type Transport = Box<dyn Transport>;
    type Udp = ();

    async fn connect(&self, url: &Url) -> io::Result<Box<dyn Transport>> {
        self.0.connect(url).await
    }

    fn arced(self) -> ArcedConnector {
        self
    }

    fn runtime(&self) -> Self::Runtime {
        self.0.runtime()
    }

    async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
        self.0.resolve(host, port).await
    }

    async fn connect_to(&self, destination: Destination) -> io::Result<Box<dyn Transport>> {
        self.0.connect_to(destination).await
    }
}

/// Factory for creating client-side QUIC endpoints.
///
/// Parameterised over `C: Connector` so that the concrete runtime and UDP socket types
/// are available to the implementation without coupling the QUIC library to any specific
/// runtime adapter.
///
/// Implementations should produce a [`QuicEndpoint`](crate::QuicEndpoint) bound to the
/// given local address. TLS configuration is embedded in the implementation.
pub trait QuicClientConfig<C: Connector>: Send + Sync + 'static {
    /// The endpoint type produced by [`bind`](QuicClientConfig::bind).
    type Endpoint: crate::QuicEndpoint;

    /// Bind a QUIC endpoint to the given local address.
    ///
    /// `runtime` is the runtime from the paired [`Connector`]; use it for spawning,
    /// timers, and UDP I/O.
    fn bind(&self, addr: SocketAddr, runtime: &C::Runtime) -> io::Result<Self::Endpoint>;
}

// -- Type-erased QuicClientConfig --

trait ObjectSafeQuicClientConfig: Send + Sync + 'static {
    fn bind(&self, addr: SocketAddr) -> io::Result<crate::ArcedQuicEndpoint>;
}

/// Binds a [`QuicClientConfig<C>`] together with its runtime before type erasure.
struct BoundQuicClientConfig<Q, C: Connector> {
    config: Q,
    runtime: C::Runtime,
}

impl<C: Connector, Q: QuicClientConfig<C>> ObjectSafeQuicClientConfig
    for BoundQuicClientConfig<Q, C>
{
    fn bind(&self, addr: SocketAddr) -> io::Result<crate::ArcedQuicEndpoint> {
        let endpoint = self.config.bind(addr, &self.runtime)?;
        Ok(crate::ArcedQuicEndpoint::from(endpoint))
    }
}

/// An arc-wrapped, type-erased QUIC client config (endpoint factory).
///
/// Created via [`Client::new_with_quic`](https://docs.rs/trillium-client/latest/trillium_client/struct.Client.html#method.new_with_quic), which
/// binds the connector's runtime into the wrapper before erasure.
#[derive(Clone)]
pub struct ArcedQuicClientConfig(Arc<dyn ObjectSafeQuicClientConfig>);

impl Debug for ArcedQuicClientConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ArcedQuicClientConfig").finish()
    }
}

impl ArcedQuicClientConfig {
    /// Binds `config` to the runtime from `connector` and wraps the result for type erasure.
    #[must_use]
    pub fn new<C: Connector, Q: QuicClientConfig<C>>(connector: &C, config: Q) -> Self {
        Self(Arc::new(BoundQuicClientConfig {
            runtime: connector.runtime(),
            config,
        }))
    }

    /// Create a type-erased QUIC endpoint bound to the given local address.
    pub fn bind(&self, addr: SocketAddr) -> io::Result<crate::ArcedQuicEndpoint> {
        self.0.bind(addr)
    }
}
