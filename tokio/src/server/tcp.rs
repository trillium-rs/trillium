use crate::TokioTransport;
use async_compat::Compat;
use std::{convert::TryInto, future::Future, io::Result, pin::Pin};
use tokio::{
    net::{TcpListener, TcpStream},
    spawn,
};
use trillium::Info;
use trillium_server_common::Server;

/// Tcp-only Trillium server for Tokio
#[derive(Debug)]
pub struct TokioServer(TcpListener);

impl Server for TokioServer {
    type Transport = TokioTransport<Compat<TcpStream>>;
    const DESCRIPTION: &'static str = concat!(
        " (",
        env!("CARGO_PKG_NAME"),
        " v",
        env!("CARGO_PKG_VERSION"),
        ")"
    );

    fn accept(&mut self) -> Pin<Box<dyn Future<Output = Result<Self::Transport>> + Send + '_>> {
        Box::pin(async move {
            self.0
                .accept()
                .await
                .map(|(t, _)| TokioTransport(Compat::new(t)))
        })
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
