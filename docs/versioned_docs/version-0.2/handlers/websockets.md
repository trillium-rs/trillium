## WebSocket support

[rustdocs (main)](https://docs.trillium.rs/trillium_websockets/index.html)


```rust
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
```

> 🌊 WebSockets work a lot like tide's, since I recently wrote that
> interface as well. One difference in trillium is that the websocket
> connection also contains some aspects of the original http request,
> such as request headers, the request path and method, and any state
> that has been accumulated by previous handlers in a sequence.
