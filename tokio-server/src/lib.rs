use async_compat::Compat;
use myco::{async_trait, Handler};
use myco_server_common::{handle_stream, Acceptor, Server};
use std::sync::Arc;
use tokio::{
    net::{TcpListener, TcpStream},
    runtime::Runtime,
};

pub struct TokioServer;

pub type Config<A> = myco_server_common::Config<TokioServer, A, Compat<TcpStream>>;

#[async_trait]
impl Server for TokioServer {
    type Transport = Compat<TcpStream>;

    fn run<A: Acceptor<Self::Transport>, H: Handler>(config: Config<A>, handler: H) {
        Runtime::new()
            .unwrap()
            .block_on(async move { Self::run_async(config, handler).await });
    }

    async fn run_async<A: Acceptor<Self::Transport>, H: Handler>(
        config: Config<A>,
        mut handler: H,
    ) {
        let socket_addrs = config.socket_addrs();
        let acceptor = config.acceptor();

        let listener = TcpListener::bind(&socket_addrs[..]).await.unwrap();
        log::info!("listening on {:?}", listener.local_addr().unwrap());
        handler.init().await;
        let handler = Arc::new(handler);

        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(handle_stream(
                Compat::new(socket),
                acceptor.clone(),
                handler.clone(),
            ));
        }
    }
}

pub fn run(handler: impl Handler) {
    config().run(handler)
}

pub fn config() -> Config<()> {
    Config::new()
}
