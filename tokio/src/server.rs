use async_compat::Compat;
use std::{convert::TryInto, future::Future, io::Result, net::IpAddr, pin::Pin};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio::{
    net::{TcpListener, TcpStream},
    spawn,
};
use trillium::{log_error, Info};
use trillium_server_common::Server;
#[cfg(unix)]
use trillium_server_common::{
    Binding::{self, *},
    Stopper,
};

#[derive(Debug, Clone, Copy)]
pub struct TokioServer;
pub type Config<A> = trillium_server_common::Config<TokioServer, A>;

#[cfg(unix)]
impl Server for TokioServer {
    type Listener = Binding<TcpListener, UnixListener>;
    type Transport = Binding<Compat<TcpStream>, Compat<UnixStream>>;
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
            use signal_hook_tokio::Signals;
            use tokio_stream::StreamExt;
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
        })
    }

    fn accept(
        listener: &mut Self::Listener,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Transport>> + Send + '_>> {
        Box::pin(async move {
            match listener {
                Tcp(t) => t.accept().await.map(|(t, _)| Tcp(Compat::new(t))),
                Unix(u) => u.accept().await.map(|(u, _)| Unix(Compat::new(u))),
            }
        })
    }

    fn peer_ip(transport: &Self::Transport) -> Option<IpAddr> {
        match transport {
            Tcp(transport) => transport
                .get_ref()
                .peer_addr()
                .ok()
                .map(|socket_addr| socket_addr.ip()),

            Unix(_) => None,
        }
    }

    fn info(listener: &Self::Listener) -> Info {
        match listener {
            Tcp(t) => t.local_addr().unwrap().into(),
            Unix(u) => (*format!("{:?}", u.local_addr().unwrap())).into(),
        }
    }

    fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
        spawn(fut);
    }

    fn block_on(fut: impl Future<Output = ()> + 'static) {
        crate::block_on(fut)
    }

    fn set_nodelay(transport: &mut Self::Transport, nodelay: bool) {
        if let Tcp(transport) = transport {
            log_error!(transport.get_mut().set_nodelay(nodelay));
        }
    }

    fn listener_from_tcp(tcp: std::net::TcpListener) -> Self::Listener {
        Tcp(tcp.try_into().unwrap())
    }

    fn listener_from_unix(tcp: std::os::unix::net::UnixListener) -> Self::Listener {
        Unix(tcp.try_into().unwrap())
    }

    fn clean_up(listener: Self::Listener) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        Box::pin(async move {
            if let Unix(u) = listener {
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
impl Server for TokioServer {
    type Listener = TcpListener;
    type Transport = Compat<TcpStream>;
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
        Box::pin(async move { listener.accept().await.map(|(t, _)| Compat::new(t)) })
    }

    fn listener_from_tcp(tcp: std::net::TcpListener) -> Self::Listener {
        tcp.try_into().unwrap()
    }

    fn peer_ip(transport: &Self::Transport) -> Option<IpAddr> {
        transport
            .get_ref()
            .peer_addr()
            .ok()
            .map(|socket_addr| socket_addr.ip())
    }

    fn info(listener: &Self::Listener) -> Info {
        listener.local_addr().unwrap().into()
    }

    fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
        spawn(fut);
    }

    fn block_on(fut: impl Future<Output = ()> + 'static) {
        crate::block_on(fut);
    }

    fn set_nodelay(transport: &mut Self::Transport, nodelay: bool) {
        log_error!(transport.get_mut().set_nodelay(nodelay));
    }
}
