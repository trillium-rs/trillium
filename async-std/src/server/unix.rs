use crate::{AsyncStdRuntime, AsyncStdTransport};
use async_std::{
    net::{TcpListener, TcpStream},
    os::unix::net::{UnixListener, UnixStream},
};
use std::io::Result;
use trillium::{log_error, Info};
use trillium_server_common::{
    Binding::{self, *},
    Server,
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

    fn from_tcp(tcp: std::net::TcpListener) -> Self {
        Self(Tcp(tcp.into()))
    }

    fn from_unix(tcp: std::os::unix::net::UnixListener) -> Self {
        Self(Unix(tcp.into()))
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
