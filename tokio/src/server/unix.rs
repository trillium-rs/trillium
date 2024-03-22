use crate::TokioTransport;
use async_compat::Compat;
use std::{future::Future, io::Result, pin::Pin};
use tokio::{
    net::{TcpListener, TcpStream, UnixListener, UnixStream},
    spawn,
};
use trillium::{log_error, Info};
use trillium_server_common::{
    Binding::{self, *},
    Server, Stopper,
};

/// Tcp/Unix Trillium server adapter for Tokio
#[derive(Debug)]
pub struct TokioServer(Binding<TcpListener, UnixListener>);

impl From<TcpListener> for TokioServer {
    fn from(value: TcpListener) -> Self {
        Self(Tcp(value))
    }
}

impl From<UnixListener> for TokioServer {
    fn from(value: UnixListener) -> Self {
        Self(Unix(value))
    }
}

impl Server for TokioServer {
    type Transport = Binding<TokioTransport<Compat<TcpStream>>, TokioTransport<Compat<UnixStream>>>;
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

    fn accept(&mut self) -> Pin<Box<dyn Future<Output = Result<Self::Transport>> + Send + '_>> {
        Box::pin(async move {
            match &mut self.0 {
                Tcp(t) => t
                    .accept()
                    .await
                    .map(|(t, _)| Tcp(TokioTransport(Compat::new(t)))),

                Unix(unix) => unix
                    .accept()
                    .await
                    .map(|(u, _)| Unix(TokioTransport(Compat::new(u)))),
            }
        })
    }

    fn info(&self) -> Info {
        match &self.0 {
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

    fn listener_from_tcp(tcp: std::net::TcpListener) -> Self {
        Self(Tcp(tcp.try_into().unwrap()))
    }

    fn listener_from_unix(unix: std::os::unix::net::UnixListener) -> Self {
        Self(Unix(unix.try_into().unwrap()))
    }

    fn clean_up(self) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        Box::pin(async move {
            if let Unix(u) = self.0 {
                if let Ok(local) = u.local_addr() {
                    if let Some(path) = local.as_pathname() {
                        log::info!("deleting {:?}", &path);
                        log_error!(tokio::fs::remove_file(path).await);
                    }
                }
            }
        })
    }
}
