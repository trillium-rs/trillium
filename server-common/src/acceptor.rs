use crate::{async_trait, Transport};
use std::{convert::Infallible, fmt::Debug};

/**
This trait provides the common interface for server-side tls
acceptors, abstracting over various implementations

The only implementation provided by this crate is `()`, which is
a noop acceptor, and passes through the `Input` type.

Implementing this trait looks like:
```rust,ignore
use trillium_server_common::{AsyncRead, AsyncWrite, async_trait, Transport};
#[async_trait]
impl<Input> Acceptor<Input> for my_tls_impl::Acceptor
where
    Input: Transport,
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
    Input: Transport,
{
    /// The stream type. For example, TlsStream<Input>
    type Output: Transport;

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
    Input: Transport,
{
    type Output = Input;
    type Error = Infallible;
    async fn accept(&self, input: Input) -> Result<Self::Output, Self::Error> {
        Ok(input)
    }
}
