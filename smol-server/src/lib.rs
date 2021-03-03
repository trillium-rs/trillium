use async_net::{TcpListener, TcpStream};
use myco::{async_trait, Handler};
use myco_server_common::{handle_stream, Acceptor};
use smol::prelude::*;
use std::sync::Arc;

pub use myco_server_common::Server;
pub type Config<A> = myco_server_common::Config<SmolServer, A, TcpStream>;

pub struct SmolServer;

#[async_trait]
impl Server for SmolServer {
    type Transport = TcpStream;

    fn run<A: Acceptor<Self::Transport>, H: Handler>(config: Config<A>, handler: H) {
        smol::block_on(async move { Self::run_async(config, handler).await })
    }

    async fn run_async<A: Acceptor<Self::Transport>, H: Handler>(
        config: Config<A>,
        mut handler: H,
    ) {
        let socket_addrs = config.socket_addrs();
        let acceptor = config.acceptor();
        let listener = TcpListener::bind(&socket_addrs[..]).await.unwrap();

        log::info!("listening on {:?}", listener.local_addr().unwrap());
        let mut incoming = listener.incoming();
        handler.init().await;
        let handler = Arc::new(handler);

        while let Some(Ok(stream)) = incoming.next().await {
            myco::log_error!(stream.set_nodelay(config.nodelay()));
            smol::spawn(handle_stream(stream, acceptor.clone(), handler.clone())).detach();
        }
    }
}

pub fn run(handler: impl Handler) {
    config().run(handler)
}

pub fn config() -> Config<()> {
    Config::new()
}
