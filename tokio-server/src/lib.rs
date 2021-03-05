use async_compat::Compat;
use futures::stream::StreamExt;
use myco::{async_trait, Handler};
use myco_server_common::{Acceptor, Server, Stopper};
use std::sync::Arc;
use tokio::{
    net::{TcpListener, TcpStream},
    runtime::Runtime,
};
use tokio_stream::wrappers::TcpListenerStream;

#[cfg(unix)]
async fn handle_signals(stop: Stopper) {
    use signal_hook::consts::signal::*;
    use signal_hook_tokio::Signals;
    let signals = Signals::new(&[SIGINT, SIGTERM, SIGQUIT]).unwrap();
    let mut signals = signals.fuse();

    while signals.next().await.is_some() {
        if stop.is_stopped() {
            println!("second interrupt, shutting down harshly");
            std::process::exit(1);
        } else {
            println!("shutting down gracefully");
            stop.stop();
        }
    }
}

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
        if config.should_register_signals() {
            #[cfg(unix)]
            tokio::spawn(handle_signals(config.stopper()));
            #[cfg(not(unix))]
            panic!("signals handling not supported on windows yet");
        }

        let socket_addrs = config.socket_addrs();
        let listener = TcpListener::bind(&socket_addrs[..]).await.unwrap();
        log::info!("listening on {:?}", listener.local_addr().unwrap());
        handler.init().await;
        let handler = Arc::new(handler);

        let mut stream = config
            .stopper()
            .stop_stream(TcpListenerStream::new(listener));

        while let Some(Ok(stream)) = stream.next().await {
            myco::log_error!(stream.set_nodelay(config.nodelay()));
            tokio::spawn(
                config
                    .clone()
                    .handle_stream(Compat::new(stream), handler.clone()),
            );
        }

        config.graceful_shutdown().await;
    }
}

pub fn run(handler: impl Handler) {
    config().run(handler)
}

pub fn config() -> Config<()> {
    Config::new()
}

pub async fn run_async(handler: impl Handler) {
    config().run_async(handler).await
}
