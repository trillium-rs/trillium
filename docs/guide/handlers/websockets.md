# WebSockets

[rustdocs](https://docs.trillium.rs/trillium_websockets)

The `trillium-websockets` crate upgrades HTTP connections to WebSocket. The handler has access to the original request headers, path, method, and any conn state accumulated by earlier handlers — so authentication and routing decisions made before the upgrade are still available inside the WebSocket handler.

```rust
# [dependencies]
# trillium-smol = { path = "../smol" }
# trillium-logger = { path = "../logger" }
# trillium-websockets = { path = "../websockets" }
# futures-lite = "*"
# env_logger = "*"
# log = "*"
#
use futures_lite::StreamExt;
use trillium_logger::logger;
use trillium_websockets::{Message, WebSocketConn, websocket};

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

## Accessing request context

Because the WebSocket handler receives the full `Upgrade` which retains the original request's state, you can read cookies, session data, route parameters, or any other state set by upstream handlers:

```rust
# [dependencies]
# trillium-smol = { path = "../smol" }
# trillium-websockets = { path = "../websockets" }
# futures-lite = "*"
#
# use futures_lite::StreamExt;
use trillium_websockets::{Message, WebSocket, WebSocketConn};

# fn main() {
#     trillium_smol::run(
WebSocket::new(|mut conn: WebSocketConn| async move {
    // Route params, cookie values, session data — all available here
    let path = conn.path().to_string();
    conn.send_string(format!("connected on {path}")).await;

    while let Some(Ok(msg)) = conn.next().await {
        if let Message::Text(text) = msg {
            conn.send_string(text.to_string()).await; // echo
        }
    }
})
# );}
```

## Client-side WebSocket

The `trillium-client` crate can upgrade connections to WebSocket with the `websockets` feature. See the [HTTP Client](./http_client.md) page.
