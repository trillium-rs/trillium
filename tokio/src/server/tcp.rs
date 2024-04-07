use crate::TokioTransport;
use async_compat::Compat;
use std::{future::Future, io::Result};
use tokio::{
    net::{TcpListener, TcpStream},
    spawn,
};
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
    type Transport = TokioTransport<Compat<TcpStream>>;
    const DESCRIPTION: &'static str = concat!(
        " (",
        env!("CARGO_PKG_NAME"),
        " v",
        env!("CARGO_PKG_VERSION"),
        ")"
    );

    async fn accept(&mut self) -> Result<Self::Transport> {
        self.0
            .accept()
            .await
            .map(|(t, _)| TokioTransport(Compat::new(t)))
    }

    fn listener_from_tcp(tcp: std::net::TcpListener) -> Self {
        Self(tcp.try_into().unwrap())
    }

    fn info(&self) -> Info {
        self.0.local_addr().unwrap().into()
    }

    fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
        spawn(fut);
    }

    fn block_on(fut: impl Future<Output = ()> + 'static) {
        crate::block_on(fut);
    }
}
