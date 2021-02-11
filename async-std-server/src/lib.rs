use async_std::net::{TcpListener, TcpStream, ToSocketAddrs};
use async_std::{prelude::*, task};
use myco::Handler;
use myco_server_common::{handle_stream, Acceptor};
use std::sync::Arc;

pub async fn run_async(
    socket_addrs: impl ToSocketAddrs,
    acceptor: impl Acceptor<TcpStream>,
    mut handler: impl Handler,
) {
    let listener = TcpListener::bind(socket_addrs).await.unwrap();
    log::info!("listening on {:?}", listener.local_addr().unwrap());
    let mut incoming = listener.incoming();
    handler.init().await;
    let handler = Arc::new(handler);

    while let Some(Ok(stream)) = incoming.next().await {
        task::spawn(handle_stream(stream, acceptor.clone(), handler.clone()));
    }
}

pub fn run(
    socket_addrs: impl ToSocketAddrs,
    acceptor: impl Acceptor<TcpStream>,
    handler: impl Handler,
) {
    task::block_on(async move { run_async(socket_addrs, acceptor, handler).await });
}
