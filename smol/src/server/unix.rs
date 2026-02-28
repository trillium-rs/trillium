use crate::{SmolRuntime, SmolTransport};
use async_net::{
    TcpListener, TcpStream,
    unix::{UnixListener, UnixStream},
};
use std::io::Result;
use trillium::{Info, log_error};
use trillium_server_common::{
    Binding::{self, *},
    Server,
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
    type Runtime = SmolRuntime;
    type Transport = Binding<SmolTransport<TcpStream>, SmolTransport<UnixStream>>;

    fn runtime() -> Self::Runtime {
        SmolRuntime::default()
    }

    async fn accept(&mut self) -> Result<Self::Transport> {
        match &self.0 {
            Tcp(t) => t.accept().await.map(|(t, _)| Tcp(SmolTransport::from(t))),
            Unix(u) => u.accept().await.map(|(u, _)| Unix(SmolTransport::from(u))),
        }
    }

    fn from_tcp(tcp: std::net::TcpListener) -> Self {
        Self(Tcp(tcp.try_into().unwrap()))
    }

    fn from_unix(tcp: std::os::unix::net::UnixListener) -> Self {
        Self(Unix(tcp.try_into().unwrap()))
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
