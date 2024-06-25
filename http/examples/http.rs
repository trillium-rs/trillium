use async_net::{TcpListener, TcpStream};
use futures_lite::prelude::*;
use trillium_http::{Conn, Swansong};

async fn handler(mut conn: Conn<TcpStream>) -> Conn<TcpStream> {
    conn.set_status(200);
    conn.set_response_body("ok");
    conn
}

pub fn main() {
    env_logger::init();

    smol::block_on(async move {
        let swansong = Swansong::new();
        let port = std::env::var("PORT")
            .unwrap_or("8080".into())
            .parse::<u16>()
            .unwrap();

        let listener = TcpListener::bind(("0.0.0.0", port)).await.unwrap();
        let mut incoming = swansong.interrupt(listener.incoming());
        while let Some(Ok(stream)) = incoming.next().await {
            let swansong = swansong.clone();
            smol::spawn(async move {
                match Conn::map(stream, swansong, handler).await {
                    Ok(Some(_)) => log::info!("upgrade"),
                    Ok(None) => log::info!("closing connection"),
                    Err(e) => log::error!("{:?}", e),
                }
            })
            .detach()
        }
    });
}
