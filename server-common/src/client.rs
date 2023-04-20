use trillium_http::transport::BoxedTransport;

use crate::{async_trait, Transport, Url};
use std::{
    fmt::{self, Debug},
    future::Future,
    io::Result,
    pin::Pin,
    sync::Arc,
};
/**
Interface for runtime and tls adapters for the trillium client

See
[`trillium_client`](https://docs.trillium.rs/trillium_client) for more
information on usage.
*/
#[async_trait]
pub trait Connector: Send + Sync + 'static {
    ///
    type Transport: Transport;
    /**
    Initiate a connection to the provided url, using the configuration.

    Async trait signature:
    ```rust,ignore
    async fn connect(url: &Url, config: &Self::Config) -> std::io::Result<Self::Transport>;
    ```
     */
    async fn connect(&self, url: &Url) -> Result<Self::Transport>;

    ///
    fn spawn<Fut: Future<Output = ()> + Send + 'static>(&self, fut: Fut);
}

///
#[async_trait]
pub trait ObjectSafeConnector: Send + Sync + 'static {
    ///
    async fn connect(&self, url: &Url) -> Result<BoxedTransport>;
    ///
    fn spawn(&self, fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>);
    ///
    fn boxed(self) -> Box<dyn ObjectSafeConnector>
    where
        Self: Sized,
    {
        Box::new(self) as Box<dyn ObjectSafeConnector>
    }

    ///
    fn arced(self) -> Arc<dyn ObjectSafeConnector>
    where
        Self: Sized,
    {
        Arc::new(self) as Arc<dyn ObjectSafeConnector>
    }
}

#[async_trait]
impl<T: Connector> ObjectSafeConnector for T {
    async fn connect(&self, url: &Url) -> Result<BoxedTransport> {
        T::connect(self, url).await.map(BoxedTransport::new)
    }

    fn spawn(&self, fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) {
        T::spawn(self, fut)
    }
}

#[async_trait]
impl Connector for Box<dyn ObjectSafeConnector> {
    type Transport = BoxedTransport;
    async fn connect(&self, url: &Url) -> Result<BoxedTransport> {
        ObjectSafeConnector::connect(self.as_ref(), url).await
    }

    fn spawn<Fut: Future<Output = ()> + Send + 'static>(&self, fut: Fut) {
        ObjectSafeConnector::spawn(self.as_ref(), Box::pin(fut))
    }
}

#[async_trait]
impl Connector for Arc<dyn ObjectSafeConnector> {
    type Transport = BoxedTransport;
    async fn connect(&self, url: &Url) -> Result<BoxedTransport> {
        ObjectSafeConnector::connect(self.as_ref(), url).await
    }

    fn spawn<Fut: Future<Output = ()> + Send + 'static>(&self, fut: Fut) {
        ObjectSafeConnector::spawn(self.as_ref(), Box::pin(fut))
    }
}

impl Debug for dyn ObjectSafeConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Arc<dyn ObjectSafeConnector>").finish()
    }
}
