use crate::{SmolRuntime, SmolTransport};
use async_net::{TcpListener, TcpStream};
use std::{convert::TryInto, io::Result, net};
use trillium::Info;
use trillium_server_common::Server;

#[derive(Debug)]
pub struct SmolTcpServer(TcpListener);
impl From<TcpListener> for SmolTcpServer {
    fn from(value: TcpListener) -> Self {
        Self(value)
    }
}

impl Server for SmolTcpServer {
    type Runtime = SmolRuntime;
    type Transport = SmolTransport<TcpStream>;

    async fn accept(&mut self) -> Result<Self::Transport> {
        self.0.accept().await.map(|(t, _)| t.into())
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
        SmolRuntime::default()
    }
}
