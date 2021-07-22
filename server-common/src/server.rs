use crate::Config;
use std::net::IpAddr;
use trillium::{async_trait, Handler};
use trillium_http::transport::Transport;
use trillium_tls_common::Acceptor;

/**
The server trait, for standard network-based server implementations.
*/
#[async_trait]
pub trait Server: Sized {
    /**
    The [`Transport`] type to use for this server implementation. This
    is often something like a TcpStream.
    */
    type Transport: Transport;

    /**
    Determine the remote ip/port for the Transport, if applicable.
     */
    #[allow(unused_variables)]
    fn peer_ip(transport: &Self::Transport) -> Option<IpAddr> {
        None
    }
    /**
    Set tcp_nodelay on the transport, if applicable.
     */
    #[allow(unused_variables)]
    fn set_nodelay(transport: &mut Self::Transport, nodelay: bool) {}

    /**
    entrypoint to run a handler with a given config. if this function
    is called, it is safe to assume that we are not yet within an
    async runtime's block_on. generally this should just entail a call
    to `my_runtime::block_on(Self::run_async(config, handler))`
    */

    fn run<A, H>(config: Config<Self, A>, handler: H)
    where
        A: Acceptor<Self::Transport>,
        H: Handler;

    /**
    entrypoint to run a handler with a given config within a
    preexisting runtime
    */

    async fn run_async<A, H>(config: Config<Self, A>, handler: H)
    where
        A: Acceptor<Self::Transport>,
        H: Handler;
}
