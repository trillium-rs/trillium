use crate::Config;
use myco::{async_trait, Handler, Transport};
use myco_tls_common::Acceptor;

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
