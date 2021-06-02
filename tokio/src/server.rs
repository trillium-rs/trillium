use async_compat::Compat;
use futures::stream::StreamExt;
use std::sync::Arc;
use tokio::{
    net::{TcpListener, TcpStream},
    runtime::Runtime,
};
use tokio_stream::wrappers::TcpListenerStream;
use trillium::{async_trait, Handler};
use trillium_server_common::{Acceptor, ConfigExt, Server};

use trillium_server_common::Stopper;

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

#[derive(Debug, Clone, Copy)]
pub struct TokioServer;
pub type Config<A> = trillium_server_common::Config<TokioServer, A>;

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

        let listener = config.build_listener::<TcpListener>();
        handler.init().await;
        let handler = Arc::new(handler);

        let mut stream = config
            .stopper()
            .stop_stream(TcpListenerStream::new(listener));

        while let Some(Ok(stream)) = stream.next().await {
            trillium::log_error!(stream.set_nodelay(config.nodelay()));
            tokio::spawn(
                config
                    .clone()
                    .handle_stream(Compat::new(stream), handler.clone()),
            );
        }

        config.graceful_shutdown().await;
    }
}
