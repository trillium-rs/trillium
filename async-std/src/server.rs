#[cfg(unix)]
use async_std::os::unix::net::{UnixListener, UnixStream};
use async_std::{
    net::{TcpListener, TcpStream},
    prelude::*,
    task::{block_on, spawn},
};
use std::{convert::TryInto, env, io::Result, net::IpAddr, pin::Pin};
use trillium::{log_error, Info};
use trillium_server_common::Server;
#[cfg(unix)]
use trillium_server_common::{
    Binding::{self, *},
    Stopper,
};

#[derive(Debug, Clone, Copy)]
pub struct AsyncStdServer;
pub type Config<A> = trillium_server_common::Config<AsyncStdServer, A>;

#[cfg(unix)]
impl Server for AsyncStdServer {
    type Listener = Binding<TcpListener, UnixListener>;
    type Transport = Binding<TcpStream, UnixStream>;
    const DESCRIPTION: &'static str = concat!(
        " (",
        env!("CARGO_PKG_NAME"),
        " v",
        env!("CARGO_PKG_VERSION"),
        ")"
    );

    fn handle_signals(stop: Stopper) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        Box::pin(async move {
            use signal_hook::consts::signal::*;
            use signal_hook_async_std::Signals;

            let signals = Signals::new([SIGINT, SIGTERM, SIGQUIT]).unwrap();
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
        })
    }

    fn accept(
        listener: &mut Self::Listener,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Transport>> + Send + '_>> {
        Box::pin(async move {
            match listener {
                Tcp(t) => t.accept().await.map(|(t, _)| Tcp(t)),
                Unix(u) => u.accept().await.map(|(u, _)| Unix(u)),
            }
        })
    }

    fn peer_ip(transport: &Self::Transport) -> Option<IpAddr> {
        match transport {
            Tcp(transport) => transport
                .peer_addr()
                .ok()
                .map(|socket_addr| socket_addr.ip()),

            Unix(_) => None,
        }
    }

    fn listener_from_tcp(tcp: std::net::TcpListener) -> Self::Listener {
        Tcp(tcp.try_into().unwrap())
    }

    fn listener_from_unix(tcp: std::os::unix::net::UnixListener) -> Self::Listener {
        Unix(tcp.try_into().unwrap())
    }

    fn info(listener: &Self::Listener) -> Info {
        match listener {
            Tcp(t) => t.local_addr().unwrap().into(),
            Unix(u) => u.local_addr().unwrap().into(),
        }
    }

    fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
        spawn(fut);
    }

    fn block_on(fut: impl Future<Output = ()> + 'static) {
        block_on(fut)
    }

    fn set_nodelay(transport: &mut Self::Transport, nodelay: bool) {
        if let Tcp(transport) = transport {
            log_error!(transport.set_nodelay(nodelay));
        }
    }

    fn clean_up(listener: Self::Listener) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        Box::pin(async move {
            if let Unix(u) = &listener {
                if let Ok(local) = u.local_addr() {
                    if let Some(path) = local.as_pathname() {
                        log::info!("deleting {:?}", &path);
                        log_error!(std::fs::remove_file(path));
                    }
                }
            }
        })
    }
}

#[cfg(not(unix))]
impl Server for AsyncStdServer {
    type Listener = TcpListener;
    type Transport = TcpStream;
    const DESCRIPTION: &'static str = concat!(
        " (",
        env!("CARGO_PKG_NAME"),
        " v",
        env!("CARGO_PKG_VERSION"),
        ")"
    );

    fn accept(
        listener: &mut Self::Listener,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Transport>> + Send + '_>> {
        Box::pin(async move { listener.accept().await.map(|(t, _)| t) })
    }

    fn peer_ip(transport: &Self::Transport) -> Option<IpAddr> {
        transport
            .peer_addr()
            .ok()
            .map(|socket_addr| socket_addr.ip())
    }

    fn listener_from_tcp(tcp: std::net::TcpListener) -> Self::Listener {
        tcp.try_into().unwrap()
    }

    fn info(listener: &Self::Listener) -> Info {
        listener.local_addr().unwrap().into()
    }

    fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
        spawn(fut);
    }

    fn block_on(fut: impl Future<Output = ()> + 'static) {
        block_on(fut);
    }

    fn set_nodelay(transport: &mut Self::Transport, nodelay: bool) {
        log_error!(transport.set_nodelay(nodelay));
    }
}
