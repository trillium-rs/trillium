use crate::{AsyncStdRuntime, AsyncStdTransport};
use async_std::net::{TcpListener, TcpStream};
use std::io::Result;
use trillium::Info;
use trillium_server_common::Server;

/// Tcp-only Trillium server for Async-std
#[derive(Debug)]
pub struct AsyncStdServer(TcpListener);
impl From<TcpListener> for AsyncStdServer {
    fn from(value: TcpListener) -> Self {
        Self(value)
    }
}
impl From<std::net::TcpListener> for AsyncStdServer {
    fn from(value: std::net::TcpListener) -> Self {
        TcpListener::from(value).into()
    }
}

impl Server for AsyncStdServer {
    type Runtime = AsyncStdRuntime;
    type Transport = AsyncStdTransport<TcpStream>;

    async fn accept(&mut self) -> Result<Self::Transport> {
        self.0.accept().await.map(|(t, _)| t.into())
    }

    fn from_tcp(tcp: std::net::TcpListener) -> Self {
        Self(tcp.into())
    }

    fn init(&self, info: &mut Info) {
        if let Ok(socket_addr) = self.0.local_addr() {
            info.insert_state(socket_addr);
        }
    }

    fn runtime() -> Self::Runtime {
        AsyncStdRuntime::default()
    }
}
