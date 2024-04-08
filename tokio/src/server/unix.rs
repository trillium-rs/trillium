use crate::{TokioRuntime, TokioTransport};
use async_compat::Compat;
use std::io::Result;
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use trillium::{log_error, Info};
use trillium_server_common::{
    Binding::{self, *},
    Server,
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

    fn init(&self, info: &mut Info) {
        match &self.0 {
            Tcp(t) => {
                if let Ok(socket_addr) = t.local_addr() {
                    info.insert_state(socket_addr);
                }
            }

            Unix(u) => {
                if let Ok(socket_addr) = u.local_addr() {
                    info.insert_state(socket_addr);
                }
            }
        }
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

    fn from_tcp(tcp_listener: std::net::TcpListener) -> Self {
        TcpListener::from_std(tcp_listener).unwrap().into()
    }

    fn from_unix(unix_listener: std::os::unix::net::UnixListener) -> Self {
        UnixListener::from_std(unix_listener).unwrap().into()
    }

    fn runtime() -> Self::Runtime {
        TokioRuntime::default()
    }
}
