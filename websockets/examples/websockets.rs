use futures_util::StreamExt;
use trillium_websockets::{websocket, Message, WebSocketConn};

async fn websocket_handler(mut conn: WebSocketConn) {
    while let Some(Ok(Message::Text(input))) = conn.next().await {
        conn.send_string(format!("received your message: {}", &input))
            .await;
    }
}

pub fn main() {
    env_logger::init();
    trillium_smol::run(websocket(websocket_handler));
}
