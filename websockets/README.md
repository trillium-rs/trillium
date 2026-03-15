# 🔌 trillium-websockets — WebSocket handler

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-websockets.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-websockets
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-websockets

WebSocket handler for Trillium, built on [`async-tungstenite`](https://docs.rs/async-tungstenite). Accepts WebSocket upgrade requests and hands each connection to an async handler function or a `WebSocketHandler` implementation. Optionally supports the `json` feature for typed JSON message handling via `JsonWebSocketHandler`.

## Example

```rust,no_run
use futures_lite::stream::StreamExt;
use trillium_websockets::{Message, WebSocketConn, websocket};

let app = websocket(|mut conn: WebSocketConn| async move {
    while let Some(Ok(Message::Text(input))) = conn.next().await {
        conn.send_string(format!("received: {input}")).await;
    }
});
// run with your chosen runtime adapter, e.g.:
// trillium_tokio::run(app);
```

## Safety

This crate uses `#![forbid(unsafe_code)]`.

## License

<sup>
Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.
</sup>

---

<sub>
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
</sub>
