use async_net::{AsyncToSocketAddrs, TcpListener, TcpStream};
use myco::Handler;
use myco_server_common::{handle_stream, Acceptor};
use smol::prelude::*;
use std::sync::Arc;

pub async fn run_async(
    socket_addrs: impl AsyncToSocketAddrs,
    acceptor: impl Acceptor<TcpStream>,
    mut handler: impl Handler,
) {
    let listener = TcpListener::bind(socket_addrs).await.unwrap();
    log::info!("listening on {:?}", listener.local_addr().unwrap());
    let mut incoming = listener.incoming();
    handler.init().await;
    let handler = Arc::new(handler);

    while let Some(Ok(stream)) = incoming.next().await {
        smol::spawn(handle_stream(stream, acceptor.clone(), handler.clone())).detach();
    }
}

pub fn run(
    socket_addrs: impl AsyncToSocketAddrs,
    acceptor: impl Acceptor<TcpStream>,
    handler: impl Handler,
) {
    smol::block_on(async move { run_async(socket_addrs, acceptor, handler).await })
}
