use crate::Transport;
use std::{borrow::Cow, convert::Infallible, fmt::Debug, future::Future};

/// This trait provides the common interface for server-side tls
/// acceptors, abstracting over various implementations
///
/// The only implementation provided by this crate is `()`, which is
/// a noop acceptor, and passes through the `Input` type.
pub trait Acceptor<Input>: Clone + Send + Sync + 'static
where
    Input: Transport,
{
    /// The stream type. For example, `TlsStream<Input>`
    type Output: Transport;

    /// An error type that [`Acceptor::accept`] may return
    type Error: Debug + Send + Sync;

    /// Transform an Input (`AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static`) into
    /// Self::Output
    fn accept(
        &self,
        input: Input,
    ) -> impl Future<Output = Result<Self::Output, Self::Error>> + Send;

    /// should conns be treated as secure?
    fn is_secure(&self) -> bool {
        true
    }

    /// The protocol negotiated via TLS ALPN with the peer for this accepted connection, if any.
    ///
    /// Used by the runtime adapter to dispatch between HTTP/1.1 and HTTP/2 on the same TCP
    /// listener: `Some(b"h2")` → HTTP/2, anything else → HTTP/1.1. Returns `None` for non-TLS
    /// acceptors and for TLS acceptors whose peers didn't negotiate an ALPN protocol.
    ///
    /// The returned [`Cow`] borrows from `output` when the backend exposes the protocol as a
    /// borrowed slice (e.g. rustls) and is owned when the backend copies (e.g. native-tls). The
    /// default returns `None` so acceptors that don't speak TLS — or don't want to opt into
    /// ALPN-based dispatch — don't need to implement it.
    fn negotiated_alpn<'a>(&self, _output: &'a Self::Output) -> Option<Cow<'a, [u8]>> {
        None
    }
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

    fn is_secure(&self) -> bool {
        false
    }
}
