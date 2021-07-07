use async_compat::Compat;
use futures_lite::prelude::*;
use tokio::net::{TcpListener, TcpStream};
use trillium_http::{Conn, Stopper};

async fn handler(mut conn: Conn<Compat<TcpStream>>) -> Conn<Compat<TcpStream>> {
    let mut body = conn.request_body().await;
    while let Some(chunk) = body.next().await {
        log::info!("< {}", String::from_utf8(chunk).unwrap());
    }
    conn.set_response_body("Hello world");
    conn.response_headers_mut()
        .insert("Content-type", "text/plain");
    conn.set_status(200);
    conn
}

#[tokio::main]
pub async fn main() {
    env_logger::init();
    let stopper = Stopper::new();
    let listener = TcpListener::bind("127.0.0.1:8081").await.unwrap();
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let stopper = stopper.clone();
                tokio::spawn(async move {
                    match Conn::map(Compat::new(stream), stopper, handler).await {
                        Ok(Some(_)) => log::info!("upgrade"),
                        Ok(None) => log::info!("closing connection"),
                        Err(e) => log::error!("{:?}", e),
                    }
                });
            }

            Err(e) => log::error!("{:?}", e),
        }
    }
}
