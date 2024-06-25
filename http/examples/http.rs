use async_net::{TcpListener, TcpStream};
use futures_lite::prelude::*;
use std::sync::Arc;
use trillium_http::{Conn, ServerConfig};

async fn handler(mut conn: Conn<TcpStream>) -> Conn<TcpStream> {
    conn.set_status(200);
    conn.set_response_body("ok");
    conn
}

pub fn main() {
    env_logger::init();

    smol::block_on(async move {
        let server_config = Arc::new(ServerConfig::new());
        let port = std::env::var("PORT")
            .unwrap_or("8080".into())
            .parse::<u16>()
            .unwrap();

        let listener = TcpListener::bind(("0.0.0.0", port)).await.unwrap();
        let mut incoming = server_config.swansong().interrupt(listener.incoming());
        while let Some(Ok(stream)) = incoming.next().await {
            let server_config = Arc::clone(&server_config);
            smol::spawn(async move {
                match server_config.run(stream, handler).await {
                    Ok(Some(_)) => log::info!("upgrade"),
                    Ok(None) => log::info!("closing connection"),
                    Err(e) => log::error!("{:?}", e),
                }
            })
            .detach()
        }
    });
}
