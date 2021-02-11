use futures_util::StreamExt;
use myco_websockets::{Message, WebSocket};

pub fn main() {
    env_logger::init();

    myco_smol_server::run(
        "localhost:8000",
        (),
        WebSocket::new(|mut websocket| async move {
            while let Some(Ok(Message::Text(input))) = websocket.next().await {
                websocket
                    .send_string(format!("received your message: {}", &input))
                    .await;
            }
        }),
    );
}
