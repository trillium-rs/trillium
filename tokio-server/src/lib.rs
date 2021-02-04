use async_compat::Compat;
use myco::Grain;
use myco_server_common::{handle_stream, Acceptor};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::{
    net::{TcpListener, ToSocketAddrs},
    runtime::Runtime,
};

pub async fn run_async(
    socket_addrs: impl ToSocketAddrs,
    acceptor: impl Acceptor<Compat<TcpStream>>,
    mut grain: impl Grain,
) {
    let listener = TcpListener::bind(socket_addrs).await.unwrap();
    log::info!("listening on {:?}", listener.local_addr().unwrap());
    grain.init().await;
    let grain = Arc::new(grain);

    while let Ok((socket, _)) = listener.accept().await {
        tokio::spawn(handle_stream(
            Compat::new(socket),
            acceptor.clone(),
            grain.clone(),
        ));
    }
}

pub fn run(
    socket_addrs: impl ToSocketAddrs,
    acceptor: impl Acceptor<Compat<TcpStream>>,
    grain: impl Grain,
) {
    Runtime::new()
        .unwrap()
        .block_on(async move { run_async(socket_addrs, acceptor, grain).await });
}
