use crate::{AsyncStdRuntime, AsyncStdTransport};
use async_std::{
    net::{TcpListener, TcpStream},
    os::unix::net::{UnixListener, UnixStream},
    stream::StreamExt,
};
use std::{env, io::Result};
use trillium::{log_error, Info};
use trillium_server_common::{
    Binding::{self, *},
    Server, Swansong,
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
    type Runtime = AsyncStdRuntime;
    type Transport = Binding<AsyncStdTransport<TcpStream>, AsyncStdTransport<UnixStream>>;

    const DESCRIPTION: &'static str = concat!(
        " (",
        env!("CARGO_PKG_NAME"),
        " v",
        env!("CARGO_PKG_VERSION"),
        ")"
    );

    async fn handle_signals(swansong: Swansong) {
        use signal_hook::consts::signal::*;
        use signal_hook_async_std::Signals;

        let signals = Signals::new([SIGINT, SIGTERM, SIGQUIT]).unwrap();
        let mut signals = signals.fuse();
        while signals.next().await.is_some() {
            if swansong.state().is_shutting_down() {
                eprintln!("\nSecond interrupt, shutting down harshly");
                std::process::exit(1);
            } else {
                println!("\nShutting down gracefully.\nControl-C again to force.");
                swansong.shut_down();
            }
        }
    }

    async fn accept(&mut self) -> Result<Self::Transport> {
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
    }

    fn listener_from_tcp(tcp: std::net::TcpListener) -> Self {
        Self(Tcp(tcp.into()))
    }

    fn listener_from_unix(tcp: std::os::unix::net::UnixListener) -> Self {
        Self(Unix(tcp.into()))
    }

    fn info(&self) -> Info {
        match &self.0 {
            Tcp(t) => t.local_addr().unwrap().into(),
            Unix(u) => u.local_addr().unwrap().into(),
        }
    }

    fn runtime() -> Self::Runtime {
        AsyncStdRuntime::default()
    }

    async fn clean_up(self) {
        if let Unix(u) = &self.0 {
            if let Ok(local) = u.local_addr() {
                if let Some(path) = local.as_pathname() {
                    log::info!("deleting {:?}", &path);
                    log_error!(async_std::fs::remove_file(path).await);
                }
            }
        }
    }
}
