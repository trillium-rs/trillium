use futures_util::StreamExt;
use trillium_websockets::{Message, WebSocket};

pub fn main() {
    env_logger::init();

    trillium_smol::run(WebSocket::new(|mut websocket| async move {
        while let Some(Ok(Message::Text(input))) = websocket.next().await {
            websocket
                .send_string(format!("received your message: {}", &input))
                .await;
        }
    }));
}
