#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

/*!
This crate provides the common interface for server-side tls
[`Acceptor`]s and client-side [`Connector`]s, abstracting over various
implementations.
*/

pub use async_trait::async_trait;
pub use futures_lite::{AsyncRead, AsyncWrite};
use std::{convert::Infallible, fmt::Debug, future::Future};
pub use url::Url;

/**
This trait provides the common interface for server-side tls
acceptors, abstracting over various implementations

The only implementation provided by this crate is `()`, which is
a noop acceptor, and passes through the `Input` type.

Implementing this trait looks like:
```rust,ignore
use trillium_tls_common::{AsyncRead, AsyncWrite, async_trait};
#[async_trait]
impl<Input> Acceptor<Input> for my_tls_impl::Acceptor
where
    Input: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    type Output = my_tls_impl::TlsStream<Input>;
    type Error = my_tls_impl::Error;
    async fn accept(&self, input: Input) -> Result<Self::Output, Self::Error> {
        self.accept(input).await
    }
}
```
*/
#[async_trait]
pub trait Acceptor<Input>: Clone + Send + Sync + 'static
where
    Input: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    /// The stream type. For example, TlsStream<Input>
    type Output: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static;

    /// An error type that [`Acceptor::accept`] may return
    type Error: Debug + Send + Sync;

    /**
    Transform an Input (`AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static`) into Self::Output

    Async trait signature:
    ```rust,ignore
    async fn accept(&self, input: Input) -> Result<Self::Output, Self::Error>;
    ```
    */
    async fn accept(&self, input: Input) -> Result<Self::Output, Self::Error>;
}

#[async_trait]
impl<Input> Acceptor<Input> for ()
where
    Input: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    type Output = Input;
    type Error = Infallible;
    async fn accept(&self, input: Input) -> Result<Self::Output, Self::Error> {
        Ok(input)
    }
}

/**
A trait to allow [`Acceptor`] types to specify an
implementation-associated [`Connector`] for a given underlying
Connector
*/
pub trait AsConnector<C: Connector>: Send + Sync + 'static {
    /// The [`Connector`] type
    type Connector: Connector;
}

impl<C: Connector> AsConnector<C> for () {
    type Connector = C;
}

/**
Interface for runtime and tls adapters for the trillium client

See
[`trillium_client`](https://docs.trillium.rs/trillium_client) for more
information on usage.
*/
#[async_trait]
pub trait Connector: Clone + Send + Sync + 'static {
    /// The async read + write type for this connector, often a TcpStream or TlSStream
    type Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static;

    /// A type that can be used to configure this Connector. It will be passed into [`Connector::connect`].
    type Config: Debug + Default + Send + Sync + Clone;

    /// A SocketAddr representation of the other side of this connection
    fn peer_addr(transport: &Self::Transport) -> std::io::Result<std::net::SocketAddr>;

    /**
    Initiate a connection to the provided url, using the configuration.

    Async trait signature:
    ```rust,ignore
    async fn connect(url: &Url, config: &Self::Config) -> std::io::Result<Self::Transport>;
    ```
     */
    async fn connect(url: &Url, config: &Self::Config) -> std::io::Result<Self::Transport>;

    /**
    Spawn and detach a task on the runtime. Although this may seem
    unrelated to the purpose of a tcp connector, it is required as a
    workaround for the absence of async drop in order to enable
    keepalive connection pooling. TLS implementations that wrap a
    runtime implementation should call through to the inner spawn.
    */
    fn spawn<Fut>(future: Fut)
    where
        Fut: Future + Send + 'static,
        <Fut as Future>::Output: Send;
}
