use async_net::{TcpListener, TcpStream};
use futures_lite::prelude::*;
use trillium_http::{Conn, Stopper};

async fn handler(mut conn: Conn<TcpStream>) -> Conn<TcpStream> {
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

pub fn main() {
    env_logger::init();

    smol::block_on(async move {
        let stopper = Stopper::new();
        let listener = TcpListener::bind("127.0.0.1:8081").await.unwrap();
        let mut incoming = stopper.stop_stream(listener.incoming());
        while let Some(Ok(stream)) = incoming.next().await {
            let stopper = stopper.clone();
            smol::spawn(async move {
                match Conn::map(stream, stopper, handler).await {
                    Ok(Some(_)) => log::info!("upgrade"),
                    Ok(None) => log::info!("closing connection"),
                    Err(e) => log::error!("{:?}", e),
                }
            })
            .detach()
        }
    });
}
