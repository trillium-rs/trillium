use crate::Config;
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
