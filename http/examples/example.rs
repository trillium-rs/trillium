use async_net::{TcpListener, TcpStream};
use futures_lite::prelude::*;
use myco_http::{Conn, Server};
use std::sync::Arc;

async fn handler(mut conn: Conn<TcpStream>) -> Conn<TcpStream> {
    let mut body = conn.request_body().await;
    while let Some(chunk) = body.next().await {
        log::info!("< {}", String::from_utf8(chunk).unwrap());
    }
    conn.set_body("Hello world");
    conn.response_headers().insert("Content-type", "text/plain");
    conn.set_status(200);
    conn
}

pub fn main() {
    env_logger::init();
    let server = Arc::new(Server::new());
    smol::block_on(async move {
        let listener = TcpListener::bind("127.0.0.1:8081").await.unwrap();
        let mut incoming = listener.incoming();
        while let Some(Ok(stream)) = incoming.next().await {
            let server = Arc::clone(&server);
            smol::spawn(async move {
                match server.accept(stream, handler).await {
                    Ok(Some(_)) => log::info!("upgrade"),
                    Ok(None) => log::info!("closing connection"),
                    Err(e) => log::error!("{:?}", e),
                }
            })
            .detach()
        }
    });
}
