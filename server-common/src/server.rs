use crate::Config;
use trillium::{async_trait, Handler};
use trillium_http::transport::Transport;
use trillium_tls_common::Acceptor;

#[async_trait]
pub trait Server: Sized {
    type Transport: Transport;

    fn run<A, H>(config: Config<Self, A>, handler: H)
    where
        A: Acceptor<Self::Transport>,
        H: Handler;

    async fn run_async<A, H>(config: Config<Self, A>, handler: H)
    where
        A: Acceptor<Self::Transport>,
        H: Handler;
}
