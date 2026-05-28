use crate::{ArcedQuicEndpoint, QuicConfig, Server};
use std::{
    fmt::{self, Debug, Formatter},
    io,
    net::UdpSocket as StdUdpSocket,
};
use trillium::Info;

type BindFn<S> = Box<
    dyn FnOnce(StdUdpSocket, <S as Server>::Runtime, &mut Info) -> io::Result<ArcedQuicEndpoint>
        + Send,
>;

/// Type-erased, one-shot QUIC binding closure. Lets the multi-listener builder hold
/// heterogeneous [`QuicConfig`] instances behind a single concrete type. The closure
/// consumes the pre-claimed [`StdUdpSocket`] and produces an [`ArcedQuicEndpoint`] once,
/// at server spawn time.
pub struct BoxedQuicConfig<S: Server>(BindFn<S>);

impl<S: Server> BoxedQuicConfig<S> {
    /// Erase a concrete [`QuicConfig`].
    pub fn new<Q: QuicConfig<S>>(quic: Q) -> Self {
        Self(Box::new(move |socket, runtime, info| {
            quic.bind_with_socket(socket, runtime, info)
                .map(ArcedQuicEndpoint::from)
        }))
    }

    /// Consume the boxed closure to bind the QUIC endpoint over `socket`, adopting it into
    /// `runtime`.
    pub fn bind(
        self,
        socket: StdUdpSocket,
        runtime: S::Runtime,
        info: &mut Info,
    ) -> io::Result<ArcedQuicEndpoint> {
        (self.0)(socket, runtime, info)
    }
}

impl<S: Server> Debug for BoxedQuicConfig<S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoxedQuicConfig").finish_non_exhaustive()
    }
}
