use crate::SmolTransport;
use async_global_executor::{block_on, spawn};
use async_net::{TcpListener, TcpStream};
use futures_lite::prelude::*;
use std::{convert::TryInto, env, io::Result, pin::Pin};
use trillium::Info;
use trillium_server_common::Server;

#[derive(Debug)]
pub struct SmolServer(TcpListener);
impl From<TcpListener> for SmolServer {
    fn from(value: TcpListener) -> Self {
        Self(value)
    }
}

impl Server for SmolServer {
    type Transport = SmolTransport<TcpStream>;
    const DESCRIPTION: &'static str = concat!(
        " (",
        env!("CARGO_PKG_NAME"),
        " v",
        env!("CARGO_PKG_VERSION"),
        ")"
    );

    fn accept(&mut self) -> Pin<Box<dyn Future<Output = Result<Self::Transport>> + Send + '_>> {
        Box::pin(async move { self.0.accept().await.map(|(t, _)| t.into()) })
    }

    fn listener_from_tcp(tcp: std::net::TcpListener) -> Self {
        Self(tcp.try_into().unwrap())
    }

    fn info(&self) -> Info {
        self.0.local_addr().unwrap().into()
    }

    fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
        spawn(fut).detach();
    }

    fn block_on(fut: impl Future<Output = ()> + 'static) {
        block_on(fut)
    }
}
