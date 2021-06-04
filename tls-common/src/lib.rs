#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
//    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

//! This crate provides the common interface for server-side tls
//! acceptors, abstracting over various implementations

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

#[async_trait]
pub trait Connector: Clone + Send + Sync + 'static {
    type Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static;
    type Config: Debug + Default + Send + Sync + Clone;
    fn peer_addr(transport: &Self::Transport) -> std::io::Result<std::net::SocketAddr>;
    async fn connect(url: &Url, config: &Self::Config) -> std::io::Result<Self::Transport>;
    fn spawn<Fut>(future: Fut)
    where
        Fut: Future + Send + 'static,
        <Fut as Future>::Output: Send;
}
