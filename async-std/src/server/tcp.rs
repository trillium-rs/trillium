use crate::{AsyncStdRuntime, AsyncStdTransport};
use async_std::net::{TcpListener, TcpStream};
use std::{env, io::Result};
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

    const DESCRIPTION: &'static str = concat!(
        " (",
        env!("CARGO_PKG_NAME"),
        " v",
        env!("CARGO_PKG_VERSION"),
        ")"
    );

    async fn accept(&mut self) -> Result<Self::Transport> {
        self.0.accept().await.map(|(t, _)| t.into())
    }

    fn listener_from_tcp(tcp: std::net::TcpListener) -> Self {
        Self(tcp.into())
    }

    fn info(&self) -> Info {
        self.0.local_addr().unwrap().into()
    }

    fn runtime() -> Self::Runtime {
        AsyncStdRuntime::default()
    }
}
