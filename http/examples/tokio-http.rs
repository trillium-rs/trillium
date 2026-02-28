use async_compat::Compat;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use trillium_http::{Conn, ServerConfig};

async fn handler(mut conn: Conn<Compat<TcpStream>>) -> Conn<Compat<TcpStream>> {
    let body = conn.request_body().await.read_string().await.unwrap();

    conn.set_response_body(format!("Hello world:\n\n{body}"));
    conn.response_headers_mut()
        .insert("Content-type", "text/plain");
    conn.set_status(200);
    conn
}

#[tokio::main]
pub async fn main() {
    env_logger::init();
    let server_config = Arc::new(ServerConfig::new());

    let listener = TcpListener::bind("127.0.0.1:8081").await.unwrap();
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let server_config = server_config.clone();
                tokio::spawn(async move {
                    match server_config.run(Compat::new(stream), handler).await {
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
