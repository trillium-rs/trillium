# WebSockets

[rustdocs](https://docs.trillium.rs/trillium_websockets)

The `trillium-websockets` crate upgrades HTTP connections to WebSocket. The handler has access to the original request headers, path, method, and any conn state accumulated by earlier handlers — so authentication and routing decisions made before the upgrade are still available inside the WebSocket handler.

```rust,noplaypen
{{#include ../../../websockets/examples/websockets.rs}}
```

## Accessing request context

Because the WebSocket handler receives the full `Upgrade` which retains the original request's state, you can read cookies, session data, route parameters, or any other state set by upstream handlers:

```rust,noplaypen
use trillium_websockets::{Message, WebSocket, WebSocketConn};

WebSocket::new(|mut conn: WebSocketConn| async move {
    // Route params, cookie values, session data — all available here
    let path = conn.path().to_string();
    conn.send_string(format!("connected on {path}")).await;

    while let Some(Ok(msg)) = conn.next().await {
        if let Message::Text(text) = msg {
            conn.send_string(text).await; // echo
        }
    }
})
```

## Client-side WebSocket

The `trillium-client` crate can upgrade connections to WebSocket with the `websockets` feature. See the [HTTP Client](./http_client.md) page.
