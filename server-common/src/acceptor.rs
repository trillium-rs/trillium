use crate::Transport;
use std::{convert::Infallible, fmt::Debug, future::Future};

/**
This trait provides the common interface for server-side tls
acceptors, abstracting over various implementations

The only implementation provided by this crate is `()`, which is
a noop acceptor, and passes through the `Input` type.

*/
pub trait Acceptor<Input>: Clone + Send + Sync + 'static
where
    Input: Transport,
{
    /// The stream type. For example, `TlsStream<Input>`
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
    fn accept(
        &self,
        input: Input,
    ) -> impl Future<Output = Result<Self::Output, Self::Error>> + Send;
}

impl<Input> Acceptor<Input> for ()
where
    Input: Transport,
{
    type Error = Infallible;
    type Output = Input;

    async fn accept(&self, input: Input) -> Result<Self::Output, Self::Error> {
        Ok(input)
    }
}
