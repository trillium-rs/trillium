use crate::{Acceptor, Transport};
use std::{
    fmt::{self, Debug, Formatter},
    future::Future,
    pin::Pin,
    sync::Arc,
};

/// Error returned by a type-erased [`BoxedAcceptor`]. The originating acceptor's error type is not
/// preserved; its `Debug` representation is captured as a string.
#[derive(Debug)]
pub struct BoxedAcceptError(String);

impl fmt::Display for BoxedAcceptError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BoxedAcceptError {}

type BoxedTransport = Box<dyn Transport>;
type AcceptFuture = Pin<Box<dyn Future<Output = Result<BoxedTransport, BoxedAcceptError>> + Send>>;

/// Object-safe sibling of [`Acceptor`], with the associated types erased to `Box<dyn Transport>`
/// and a boxed future, so acceptors of differing concrete types can be stored behind one trait
/// object.
trait DynAcceptor<Input: Transport>: Send + Sync {
    fn dyn_accept(&self, input: Input) -> AcceptFuture;
    fn is_secure(&self) -> bool;
}

impl<Input, A> DynAcceptor<Input> for A
where
    Input: Transport,
    A: Acceptor<Input>,
{
    fn dyn_accept(&self, input: Input) -> AcceptFuture {
        // Clone so the returned future owns its acceptor and is `'static`. Acceptors are cheap to
        // clone (typically `Arc`-backed config).
        let acceptor = self.clone();
        Box::pin(async move {
            acceptor
                .accept(input)
                .await
                .map(|transport| Box::new(transport) as BoxedTransport)
                .map_err(|e| BoxedAcceptError(format!("{e:?}")))
        })
    }

    fn is_secure(&self) -> bool {
        Acceptor::is_secure(self)
    }
}

/// A type-erased [`Acceptor`]. Wrapping acceptors of differing concrete types as `BoxedAcceptor`
/// lets a single multi-listener server hold several — e.g. a plaintext listener (`()`) alongside
/// one or more TLS listeners. The erased [`Output`](Acceptor::Output) is `Box<dyn Transport>`.
///
/// The per-connection cost of erasure is one heap allocation plus a virtual dispatch at `accept`
/// time — negligible relative to a TLS handshake, and entirely off the per-request path.
pub struct BoxedAcceptor<Input>(Arc<dyn DynAcceptor<Input>>);

impl<Input: Transport> BoxedAcceptor<Input> {
    /// Erase a concrete acceptor.
    pub fn new<A: Acceptor<Input>>(acceptor: A) -> Self {
        Self(Arc::new(acceptor))
    }
}

impl<Input: Transport> Clone for BoxedAcceptor<Input> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<Input: Transport> Debug for BoxedAcceptor<Input> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoxedAcceptor")
            .field("is_secure", &self.0.is_secure())
            .finish_non_exhaustive()
    }
}

impl<Input: Transport> Acceptor<Input> for BoxedAcceptor<Input> {
    type Error = BoxedAcceptError;
    type Output = BoxedTransport;

    async fn accept(&self, input: Input) -> Result<Self::Output, Self::Error> {
        self.0.dyn_accept(input).await
    }

    fn is_secure(&self) -> bool {
        self.0.is_secure()
    }
}
