use crate::{TokioRuntime, TokioTransport};
use async_compat::Compat;
use std::{io, net};
use tokio::net::{TcpListener, TcpStream};
use trillium::Info;
use trillium_server_common::Server;

/// Tcp-only Trillium server for Tokio
#[derive(Debug)]
pub struct TokioServer(TcpListener);

impl From<TcpListener> for TokioServer {
    fn from(value: TcpListener) -> Self {
        Self(value)
    }
}

impl Server for TokioServer {
    type Runtime = TokioRuntime;
    type Transport = TokioTransport<Compat<TcpStream>>;

    async fn accept(&mut self) -> io::Result<Self::Transport> {
        self.0
            .accept()
            .await
            .map(|(t, _)| TokioTransport(Compat::new(t)))
    }

    fn from_tcp(tcp: net::TcpListener) -> Self {
        Self(tcp.try_into().unwrap())
    }

    fn init(&self, info: &mut Info) {
        if let Ok(socket_addr) = self.0.local_addr() {
            info.insert_state(socket_addr);
        }
    }

    fn runtime() -> Self::Runtime {
        TokioRuntime::default()
    }
}
