use futures_util::StreamExt;
use trillium_logger::logger;
use trillium_websockets::{websocket, Message, WebSocketConn};

async fn websocket_handler(mut conn: WebSocketConn) {
    while let Some(Ok(Message::Text(input))) = conn.next().await {
        let result = conn
            .send_string(format!("received your message: {}", &input))
            .await;

        if let Err(e) = result {
            log::error!("{e}");
            break;
        }
    }
}

pub fn main() {
    env_logger::init();
    trillium_smol::run((logger(), websocket(websocket_handler)));
}
