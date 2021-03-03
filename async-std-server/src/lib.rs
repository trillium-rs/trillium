use async_std::net::{TcpListener, TcpStream};
use async_std::{prelude::*, task};
use myco::{async_trait, Handler};
use myco_server_common::{handle_stream, Acceptor, Server};
use std::sync::Arc;

pub type Config<A> = myco_server_common::Config<AsyncStdServer, A, TcpStream>;

pub struct AsyncStdServer;

#[async_trait]
impl Server for AsyncStdServer {
    type Transport = TcpStream;

    fn run<A: Acceptor<Self::Transport>, H: Handler>(config: Config<A>, handler: H) {
        task::block_on(async move { Self::run_async(config, handler).await })
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
            task::spawn(handle_stream(stream, acceptor.clone(), handler.clone()));
        }
    }
}

pub fn run(handler: impl Handler) {
    config().run(handler)
}

pub fn config() -> Config<()> {
    Config::new()
}
