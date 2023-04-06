use crate::AsyncStdTransport;
use async_std::{
    net::{TcpListener, TcpStream},
    os::unix::net::{UnixListener, UnixStream},
    stream::StreamExt,
    task::{block_on, spawn},
};
use std::{convert::TryInto, env, future::Future, io::Result, pin::Pin};
use trillium::{log_error, Info};
use trillium_server_common::{
    Binding::{self, *},
    Server, Stopper,
};

/// Tcp/Unix Trillium server adapter for Async-Std
#[derive(Debug)]
pub struct AsyncStdServer(Binding<TcpListener, UnixListener>);
impl From<TcpListener> for AsyncStdServer {
    fn from(value: TcpListener) -> Self {
        Self(Tcp(value))
    }
}

impl From<UnixListener> for AsyncStdServer {
    fn from(value: UnixListener) -> Self {
        Self(Unix(value))
    }
}
impl From<std::net::TcpListener> for AsyncStdServer {
    fn from(value: std::net::TcpListener) -> Self {
        TcpListener::from(value).into()
    }
}
impl From<std::os::unix::net::UnixListener> for AsyncStdServer {
    fn from(value: std::os::unix::net::UnixListener) -> Self {
        UnixListener::from(value).into()
    }
}

#[cfg(unix)]
impl Server for AsyncStdServer {
    type Transport = Binding<AsyncStdTransport<TcpStream>, AsyncStdTransport<UnixStream>>;
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

    fn accept(&mut self) -> Pin<Box<dyn Future<Output = Result<Self::Transport>> + Send + '_>> {
        Box::pin(async move {
            match &self.0 {
                Tcp(t) => t
                    .accept()
                    .await
                    .map(|(t, _)| Tcp(AsyncStdTransport::from(t))),

                Unix(u) => u
                    .accept()
                    .await
                    .map(|(u, _)| Unix(AsyncStdTransport::from(u))),
            }
        })
    }

    fn listener_from_tcp(tcp: std::net::TcpListener) -> Self {
        Self(Tcp(tcp.try_into().unwrap()))
    }

    fn listener_from_unix(tcp: std::os::unix::net::UnixListener) -> Self {
        Self(Unix(tcp.try_into().unwrap()))
    }

    fn info(&self) -> Info {
        match &self.0 {
            Tcp(t) => t.local_addr().unwrap().into(),
            Unix(u) => u.local_addr().unwrap().into(),
        }
    }

    fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
        spawn(fut);
    }

    fn block_on(fut: impl Future<Output = ()> + 'static) {
        block_on(fut);
    }

    fn clean_up(self) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        Box::pin(async move {
            if let Unix(u) = &self.0 {
                if let Ok(local) = u.local_addr() {
                    if let Some(path) = local.as_pathname() {
                        log::info!("deleting {:?}", &path);
                        log_error!(async_std::fs::remove_file(path).await);
                    }
                }
            }
        })
    }
}
