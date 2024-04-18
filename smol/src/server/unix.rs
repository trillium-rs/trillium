use crate::{SmolRuntime, SmolTransport};
use async_net::{
    unix::{UnixListener, UnixStream},
    TcpListener, TcpStream,
};
use futures_lite::prelude::*;
use std::{env, io::Result};
use trillium::{log_error, Info};
use trillium_server_common::{
    Binding::{self, *},
    Server, Swansong, Url,
};

#[derive(Debug, Clone)]
pub struct SmolServer(Binding<TcpListener, UnixListener>);
impl From<TcpListener> for SmolServer {
    fn from(value: TcpListener) -> Self {
        Self(Tcp(value))
    }
}
impl From<UnixListener> for SmolServer {
    fn from(value: UnixListener) -> Self {
        Self(Unix(value))
    }
}

#[cfg(unix)]
impl Server for SmolServer {
    type Transport = Binding<SmolTransport<TcpStream>, SmolTransport<UnixStream>>;
    type Runtime = SmolRuntime;
    const DESCRIPTION: &'static str = concat!(
        " (",
        env!("CARGO_PKG_NAME"),
        " v",
        env!("CARGO_PKG_VERSION"),
        ")"
    );

    fn runtime() -> Self::Runtime {
        SmolRuntime::default()
    }

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
            Tcp(t) => t.accept().await.map(|(t, _)| Tcp(SmolTransport::from(t))),
            Unix(u) => u.accept().await.map(|(u, _)| Unix(SmolTransport::from(u))),
        }
    }

    fn listener_from_tcp(tcp: std::net::TcpListener) -> Self {
        Self(Tcp(tcp.try_into().unwrap()))
    }

    fn listener_from_unix(tcp: std::os::unix::net::UnixListener) -> Self {
        Self(Unix(tcp.try_into().unwrap()))
    }

    fn info(&self) -> Info {
        match &self.0 {
            Tcp(t) => {
                let local_addr = t.local_addr().unwrap();
                let mut info = Info::from(local_addr);
                if let Ok(url) = Url::parse(&format!("http://{local_addr}")) {
                    info.state_mut().insert(url);
                }
                info
            }
            Unix(u) => u.local_addr().unwrap().into(),
        }
    }

    async fn clean_up(self) {
        if let Unix(u) = &self.0 {
            if let Ok(local) = u.local_addr() {
                if let Some(path) = local.as_pathname() {
                    log::info!("deleting {:?}", &path);
                    log_error!(std::fs::remove_file(path));
                }
            }
        }
    }
}
