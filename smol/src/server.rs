use async_global_executor::{block_on, spawn};
use async_net::{TcpListener, TcpStream};
use futures_lite::prelude::*;
use std::sync::Arc;
use trillium::{async_trait, Handler};
use trillium_server_common::{Acceptor, ConfigExt, Server, Stopper};

#[derive(Debug, Clone, Copy)]
pub struct Smol;
pub type Config<A> = trillium_server_common::Config<Smol, A>;

#[cfg(unix)]
async fn handle_signals(stop: Stopper) {
    use signal_hook::consts::signal::*;
    use signal_hook_async_std::Signals;

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

#[async_trait]
impl Server for Smol {
    type Transport = TcpStream;

    fn run<A: Acceptor<Self::Transport>, H: Handler>(config: Config<A>, handler: H) {
        block_on(Self::run_async(config, handler))
    }

    async fn run_async<A: Acceptor<Self::Transport>, H: Handler>(
        config: Config<A>,
        mut handler: H,
    ) {
        if config.should_register_signals() {
            #[cfg(unix)]
            spawn(handle_signals(config.stopper())).detach();
            #[cfg(not(unix))]
            panic!("signals handling not supported on windows yet");
        }

        let listener = config.build_listener::<TcpListener>();
        let mut incoming = config.stopper().stop_stream(listener.incoming());
        handler.init().await;
        let handler = Arc::new(handler);

        while let Some(Ok(stream)) = incoming.next().await {
            trillium::log_error!(stream.set_nodelay(config.nodelay()));
            spawn(config.clone().handle_stream(stream, handler.clone())).detach();
        }

        config.graceful_shutdown().await;
    }
}
