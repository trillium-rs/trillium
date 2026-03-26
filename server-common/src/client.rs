use crate::{Runtime, RuntimeTrait, Transport, UdpTransport, Url};
use std::{
    any::Any,
    fmt::{self, Debug, Formatter},
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
};

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
