use async_std::{
    net::{TcpListener, TcpStream},
    prelude::*,
    task::{block_on, spawn},
};
use std::{net::IpAddr, sync::Arc};
use trillium::{async_trait, Handler, Info};
use trillium_server_common::{Acceptor, ConfigExt, Server, Stopper};

const SERVER_DESCRIPTION: &str = concat!(
    " (",
    env!("CARGO_PKG_NAME"),
    " v",
    env!("CARGO_PKG_VERSION"),
    ")"
);

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

#[derive(Debug, Clone, Copy)]
pub struct AsyncStdServer;
pub type Config<A> = trillium_server_common::Config<AsyncStdServer, A>;

#[async_trait]
impl Server for AsyncStdServer {
    type Transport = TcpStream;

    fn peer_ip(transport: &Self::Transport) -> Option<IpAddr> {
        transport
            .peer_addr()
            .ok()
            .map(|socket_addr| socket_addr.ip())
    }

    fn run<A: Acceptor<Self::Transport>, H: Handler>(config: Config<A>, handler: H) {
        block_on(async move { Self::run_async(config, handler).await })
    }

    async fn run_async<A: Acceptor<Self::Transport>, H: Handler>(
        config: Config<A>,
        mut handler: H,
    ) {
        if config.should_register_signals() {
            #[cfg(unix)]
            spawn(handle_signals(config.stopper()));
            #[cfg(not(unix))]
            panic!("signals handling not supported on windows yet");
        }

        let listener = config.build_listener::<TcpListener>();
        let local_addr = listener.local_addr().unwrap();
        let mut info = Info::from(local_addr);
        *info.listener_description_mut() = format!("http://{}:{}", config.host(), config.port());
        info.server_description_mut().push_str(SERVER_DESCRIPTION);

        let mut stream = config.stopper().stop_stream(listener.incoming());

        handler.init(&mut info).await;
        let handler = Arc::new(handler);
        while let Some(stream) = stream.next().await {
            match stream {
                Ok(stream) => {
                    let config = config.clone();
                    let handler = handler.clone();
                    spawn(config.handle_stream(stream, handler));
                }

                Err(e) => log::error!("tcp error: {}", e),
            }
        }
        config.graceful_shutdown().await;
    }

    fn set_nodelay(transport: &mut Self::Transport, nodelay: bool) {
        trillium::log_error!(transport.set_nodelay(nodelay));
    }
}
