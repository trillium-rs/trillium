use async_compat::Compat;
use myco::Handler;
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
    mut handler: impl Handler,
) {
    let listener = TcpListener::bind(socket_addrs).await.unwrap();
    log::info!("listening on {:?}", listener.local_addr().unwrap());
    handler.init().await;
    let handler = Arc::new(handler);

    while let Ok((socket, _)) = listener.accept().await {
        tokio::spawn(handle_stream(
            Compat::new(socket),
            acceptor.clone(),
            handler.clone(),
        ));
    }
}

pub fn run(
    socket_addrs: impl ToSocketAddrs,
    acceptor: impl Acceptor<Compat<TcpStream>>,
    handler: impl Handler,
) {
    Runtime::new()
        .unwrap()
        .block_on(async move { run_async(socket_addrs, acceptor, handler).await });
}
