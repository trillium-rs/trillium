use crate::{Runtime, RuntimeTrait, Transport, Url};
use std::{
    any::Any,
    fmt::{self, Debug},
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
};
use trillium_http::transport::BoxedTransport;
/**
Interface for runtime and tls adapters for the trillium client

See
[`trillium_client`](https://docs.trillium.rs/trillium_client) for more
information on usage.
*/
pub trait Connector: Send + Sync + 'static {
    /// the [`Transport`] that [`connect`] returns
    type Transport: Transport;

    /// The [`RuntimeTrait`] for this Connector
    type Runtime: RuntimeTrait;

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
    ) -> Pin<Box<dyn Future<Output = io::Result<BoxedTransport>> + Send + 'fut>>
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
    ) -> Pin<Box<dyn Future<Output = io::Result<BoxedTransport>> + Send + 'fut>>
    where
        'connector: 'fut,
        'url: 'fut,
        Self: 'fut,
    {
        Box::pin(async move { Connector::connect(self, url).await.map(BoxedTransport::new) })
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
    type Transport = BoxedTransport;
    async fn connect(&self, url: &Url) -> io::Result<BoxedTransport> {
        self.0.connect(url).await
    }

    type Runtime = Runtime;

    fn arced(self) -> ArcedConnector {
        self
    }

    fn runtime(&self) -> Self::Runtime {
        self.0.runtime()
    }
}
