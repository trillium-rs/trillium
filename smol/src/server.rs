use async_global_executor::{block_on, spawn};
use async_net::{TcpListener, TcpStream};
use futures_lite::prelude::*;
use std::{net::IpAddr, sync::Arc};
use trillium::{async_trait, log_error, Handler, Info};
use trillium_server_common::{Acceptor, ConfigExt, Server, Stopper};

const SERVER_DESCRIPTION: &str = concat!(
    " (",
    env!("CARGO_PKG_NAME"),
    " v",
    env!("CARGO_PKG_VERSION"),
    ")"
);

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
            eprintln!("\nSecond interrupt, shutting down harshly");
            std::process::exit(1);
        } else {
            println!("\nShutting down gracefully.\nControl-C again to force.");
            stop.stop();
        }
    }
}

#[async_trait]
impl Server for Smol {
    type Transport = TcpStream;

    fn peer_ip(transport: &Self::Transport) -> Option<IpAddr> {
        transport
            .peer_addr()
            .ok()
            .map(|socket_addr| socket_addr.ip())
    }

    fn set_nodelay(transport: &mut Self::Transport, nodelay: bool) {
        log_error!(transport.set_nodelay(nodelay));
    }

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

        let stream = listener.incoming();
        let mut stream = config.stopper().stop_stream(stream);

        let local_addr = listener.local_addr().unwrap();
        let mut info = Info::from(local_addr);
        *info.listener_description_mut() = format!("http://{}:{}", config.host(), config.port());
        info.server_description_mut().push_str(SERVER_DESCRIPTION);

        handler.init(&mut info).await;
        let handler = Arc::new(handler);
        while let Some(stream) = stream.next().await {
            match stream {
                Ok(stream) => spawn(config.clone().handle_stream(stream, handler.clone())).detach(),
                Err(e) => log::error!("tcp error: {}", e),
            }
        }
        config.graceful_shutdown().await;
    }
}
