use crate::{QuicConnection, Runtime, RuntimeTrait, Transport, UdpTransport, Url};
use std::{
    any::Any,
    fmt::{self, Debug},
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
};

/// Interface for runtime and tls adapters for the trillium client
///
/// See
/// [`trillium_client`](https://docs.trillium.rs/trillium_client) for more
/// information on usage.
pub trait Connector: Send + Sync + 'static {
    /// the [`Transport`] that [`connect`] returns
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

    /// Returns the runtime
    fn runtime(&self) -> Self::Runtime;
}

/// An Arced and type-erased [`Connector`]
#[derive(Clone)]
pub struct ArcedConnector(Arc<dyn ObjectSafeConnector>);

impl Debug for ArcedConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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

trait ObjectSafeConnector: Send + Sync + 'static {
    #[must_use]
    fn connect<'connector, 'url, 'fut>(
        &'connector self,
        url: &'url Url,
    ) -> Pin<Box<dyn Future<Output = io::Result<Box<dyn Transport>>> + Send + 'fut>>
    where
        'connector: 'fut,
        'url: 'fut,
        Self: 'fut;
    fn as_any(&self) -> &dyn Any;
    fn as_mut_any(&mut self) -> &mut dyn Any;
    fn runtime(&self) -> Runtime;
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
}

/// Interface for QUIC adapters for the trillium client.
///
/// Parameterised over `C: Connector` so that the concrete runtime and UDP socket types
/// are available to the implementation without coupling the QUIC library to any specific
/// runtime adapter.
///
/// Implementations receive `runtime: &C::Runtime` in [`connect`](QuicConnector::connect) —
/// the same runtime the TCP connector is using — so spawning and timers use the correct
/// executor automatically.
///
/// Implementations are expected to handle TLS internally (QUIC embeds TLS 1.3) and to
/// manage the HTTP/3 control and QPACK streams on the connection before returning.
pub trait QuicConnector<C: Connector>: Send + Sync + 'static {
    /// Establish a QUIC connection to `host:port`, returning an HTTP/3-ready connection.
    ///
    /// `runtime` is the runtime from the paired [`Connector`]; use it for any spawning
    /// or timer operations needed during connection setup.
    fn connect<'a>(
        &'a self,
        host: &'a str,
        port: u16,
        runtime: &'a C::Runtime,
    ) -> impl Future<Output = io::Result<QuicConnection>> + Send + 'a;
}

// -- Type-erased QuicConnector --

type BoxedQuicFuture<'a> = Pin<Box<dyn Future<Output = io::Result<QuicConnection>> + Send + 'a>>;

trait ObjectSafeQuicConnector: Send + Sync + 'static {
    fn connect<'a>(&'a self, host: &'a str, port: u16) -> BoxedQuicFuture<'a>;
    fn as_any(&self) -> &dyn Any;
}

/// Binds a [`QuicConnector<C>`] together with its runtime before type erasure.
///
/// Created inside [`Client::new_with_quic`](trillium_client::Client::new_with_quic);
/// not part of the public API.
struct BoundQuicConnector<Q, C: Connector> {
    quic: Q,
    runtime: C::Runtime,
}

impl<C: Connector, Q: QuicConnector<C>> ObjectSafeQuicConnector for BoundQuicConnector<Q, C> {
    fn connect<'a>(&'a self, host: &'a str, port: u16) -> BoxedQuicFuture<'a> {
        Box::pin(self.quic.connect(host, port, &self.runtime))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// An arc-wrapped, type-erased QUIC connector.
///
/// Created via [`Client::new_with_quic`](trillium_client::Client::new_with_quic), which
/// binds the connector's runtime into the wrapper before erasure. Not constructed directly.
#[derive(Clone)]
pub struct ArcedQuicConnector(Arc<dyn ObjectSafeQuicConnector>);

impl Debug for ArcedQuicConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ArcedQuicConnector").finish()
    }
}

impl ArcedQuicConnector {
    /// Binds `quic` to the runtime from `connector` and wraps the result for type erasure.
    ///
    /// Called by [`Client::new_with_quic`](trillium_client::Client::new_with_quic).
    #[must_use]
    pub fn new<C: Connector, Q: QuicConnector<C>>(connector: &C, quic: Q) -> Self {
        Self(Arc::new(BoundQuicConnector {
            runtime: connector.runtime(),
            quic,
        }))
    }

    /// Connect to `host:port`, returning an HTTP/3-ready [`QuicConnection`].
    pub async fn connect(&self, host: &str, port: u16) -> io::Result<QuicConnection> {
        self.0.connect(host, port).await
    }

    /// Determine if this `ArcedQuicConnector` wraps the specified concrete type.
    pub fn is<T: Any + 'static>(&self) -> bool {
        self.0.as_any().is::<T>()
    }

    /// Attempt to borrow the inner connector as the provided concrete type.
    pub fn downcast_ref<T: Any + 'static>(&self) -> Option<&T> {
        self.0.as_any().downcast_ref()
    }
}
