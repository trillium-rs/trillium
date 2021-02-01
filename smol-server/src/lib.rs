use async_net::{AsyncToSocketAddrs, TcpListener};
use myco::Grain;
use myco_server_common::handle_stream;
use smol::prelude::*;
use std::sync::Arc;

pub async fn run_async(socket_addrs: impl AsyncToSocketAddrs, mut grain: impl Grain) {
    let listener = TcpListener::bind(socket_addrs).await.unwrap();
    let mut incoming = listener.incoming();
    grain.init().await;
    let grain = Arc::new(grain);

    while let Some(Ok(stream)) = incoming.next().await {
        smol::spawn(handle_stream(stream, grain.clone())).detach();
    }
}

pub fn run(socket_addrs: impl AsyncToSocketAddrs, grain: impl Grain) {
    smol::block_on(async move { run_async(socket_addrs, grain).await })
}
