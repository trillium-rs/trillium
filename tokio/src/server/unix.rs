use crate::{TokioRuntime, TokioTransport};
use async_compat::Compat;
use std::io::Result;
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use trillium::{Info, log_error};
use trillium_server_common::{
    Binding::{self, *},
    Server, Swansong,
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
    type Runtime = TokioRuntime;
    type Transport = Binding<TokioTransport<Compat<TcpStream>>, TokioTransport<Compat<UnixStream>>>;

    const DESCRIPTION: &'static str = concat!(
        " (",
        env!("CARGO_PKG_NAME"),
        " v",
        env!("CARGO_PKG_VERSION"),
        ")"
    );

    async fn handle_signals(swansong: Swansong) {
        use signal_hook::consts::signal::*;
        use signal_hook_tokio::Signals;
        use tokio_stream::StreamExt;
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
    }

    fn info(&self) -> Info {
        match &self.0 {
            Tcp(t) => t.local_addr().unwrap().into(),
            Unix(u) => (*format!("{:?}", u.local_addr().unwrap())).into(),
        }
    }

    fn listener_from_tcp(tcp: std::net::TcpListener) -> Self {
        Self(Tcp(tcp.try_into().unwrap()))
    }

    fn listener_from_unix(unix: std::os::unix::net::UnixListener) -> Self {
        Self(Unix(unix.try_into().unwrap()))
    }

    async fn clean_up(self) {
        if let Unix(u) = self.0 {
            if let Ok(local) = u.local_addr() {
                if let Some(path) = local.as_pathname() {
                    log::info!("deleting {:?}", &path);
                    log_error!(tokio::fs::remove_file(path).await);
                }
            }
        }
    }

    fn runtime() -> Self::Runtime {
        TokioRuntime::default()
    }
}
