use async_compat::Compat;
use myco::Grain;
use myco_server_common::handle_stream;
use std::sync::Arc;
use tokio::{
    net::{TcpListener, ToSocketAddrs},
    runtime::Runtime,
};

pub async fn run_async(socket_addrs: impl ToSocketAddrs, mut grain: impl Grain) {
    let listener = TcpListener::bind(socket_addrs).await.unwrap();
    grain.init().await;
    let grain = Arc::new(grain);

    while let Ok((socket, _)) = listener.accept().await {
        tokio::spawn(handle_stream(Compat::new(socket), grain.clone()));
    }
}

pub fn run(socket_addrs: impl ToSocketAddrs, grain: impl Grain) {
    Runtime::new()
        .unwrap()
        .block_on(async move { run_async(socket_addrs, grain).await });
}
