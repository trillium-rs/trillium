use async_native_tls::TlsAcceptor;
use async_net::{AsyncToSocketAddrs, TcpListener};
use myco::Grain;
use myco_server_common::handle_stream;
use smol::prelude::*;
use std::sync::Arc;

pub async fn run_async(
    bind: impl AsyncToSocketAddrs,
    acceptor: impl Into<TlsAcceptor>,
    mut grain: impl Grain,
) {
    let acceptor = acceptor.into();
    let listener = TcpListener::bind(bind).await.unwrap();
    let mut incoming = listener.incoming();
    grain.init().await;
    let grain = Arc::new(grain);

    while let Some(Ok(stream)) = incoming.next().await {
        let acceptor = acceptor.clone();
        let grain = grain.clone();
        smol::spawn(async move {
            match acceptor.accept(stream).await {
                Ok(stream) => handle_stream(stream, grain).await,
                Err(e) => log::error!("tls error: {:?}", e),
            }
        })
        .detach();
    }
}

pub fn run(bind: impl AsyncToSocketAddrs, acceptor: impl Into<TlsAcceptor>, grain: impl Grain) {
    smol::block_on(async move { run_async(bind, acceptor, grain).await })
}
