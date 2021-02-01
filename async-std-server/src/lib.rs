use async_std::net::{TcpListener, ToSocketAddrs};
use async_std::{prelude::*, task};
use myco::Grain;
use myco_server_common::handle_stream;
use std::sync::Arc;

pub async fn run_async(socket_addrs: impl ToSocketAddrs, mut grain: impl Grain) {
    let listener = TcpListener::bind(socket_addrs).await.unwrap();
    let mut incoming = listener.incoming();
    grain.init().await;
    let grain = Arc::new(grain);

    while let Some(Ok(stream)) = incoming.next().await {
        task::spawn(handle_stream(stream, grain.clone()));
    }
}

pub fn run(socket_addrs: impl ToSocketAddrs, grain: impl Grain) {
    task::block_on(async move { run_async(socket_addrs, grain).await });
}
