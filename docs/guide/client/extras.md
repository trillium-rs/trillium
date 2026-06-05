# WebSockets, WebTransport, and JSON

A few client capabilities live behind cargo features, so they only pull in their dependencies when you ask for them.

## WebSocket client

With the `websockets` feature, a built conn can be upgraded to a WebSocket. This works over HTTP/1.1 (RFC 6455) and, when the connection negotiated h2, over HTTP/2 extended CONNECT (RFC 8441) — the same upgrade either way from the caller's side.

```rust
# [dependencies]
# trillium-smol = { path = "../smol" }
# trillium-client = { path = "../client", features = ["websockets"] }
# trillium-testing = { path = "../testing" }
#
# fn main() { trillium_testing::block_on(async {
# use trillium_client::Client;
# use trillium_smol::ClientConfig;
# let client = Client::new(ClientConfig::default());
let ws_conn = client
    .get("wss://example.com/ws")
    .await
    .unwrap()
    .into_websocket()
    .await
    .unwrap();
# let _ = ws_conn;
# }); }
```

The resulting `WebSocketConn` exposes the same send/receive interface as the server-side WebSocket handler.

## WebTransport client

[WebTransport](../handlers/webtransport.md) is a protocol over HTTP/3 and QUIC offering multiplexed streams and unreliable datagrams. With the `webtransport` feature, a client built with `new_with_quic` can open sessions to a WebTransport server.

`Client::webtransport(url)` builds a conn preconfigured for the extended-CONNECT handshake — method CONNECT, the `:protocol` pseudo-header set to `webtransport`, pinned to HTTP/3. Awaiting it with `Conn::into_webtransport()` completes the upgrade and hands back a `WebTransportConnection`, the same session type the server handler uses.

```rust
# [dependencies]
# trillium-tokio = { path = "../tokio" }
# trillium-client = { path = "../client", features = ["webtransport"] }
# trillium-quinn = { path = "../quinn", features = ["webpki-roots"] }
# trillium-rustls = { path = "../rustls" }
#
# fn main() {
use trillium_client::Client;
use trillium_quinn::ClientQuicConfig;
use trillium_rustls::RustlsConfig;
use trillium_tokio::ClientConfig;

let client = Client::new_with_quic(
    RustlsConfig::<ClientConfig>::default(),
    ClientQuicConfig::with_webpki_roots(),
);

let conn = client.webtransport("https://example.com/wt");
// let session = conn.into_webtransport().await?;
# let _ = conn;
# }
```

Multiple sessions to the same origin coalesce onto a single underlying QUIC connection, matching how HTTP/3 request multiplexing already works.

## JSON bodies

Enabling either the `serde_json` or `sonic-rs` feature adds JSON convenience methods backed by that serializer. `Conn::response_json::<T>()` deserializes a response body, and `Conn::with_json_body` serializes a request body:

```rust
# [dependencies]
# trillium-smol = { path = "../smol" }
# trillium-client = { path = "../client", features = ["serde_json"] }
# trillium-testing = { path = "../testing" }
# serde = { version = "1.0", features = ["derive"] }
#
# fn main() { trillium_testing::block_on(async {
use serde::Deserialize;
use trillium_client::Client;
use trillium_smol::ClientConfig;

#[derive(Deserialize)]
struct Widget {
    name: String,
}

# let client = Client::new(ClientConfig::default());
let mut conn = client.get("https://api.example.com/widget").await.unwrap();
let widget: Widget = conn.response_json().await.unwrap();
println!("{}", widget.name);
# }); }
```

JSON errors surface as `ClientSerdeError`, which wraps either a transport error or a serializer error. For ad-hoc request bodies without a struct, the crate re-exports a `json!` macro. The two backends are mutually exclusive — enable one.

## Also: gRPC

[`trillium-grpc`](https://docs.rs/trillium-grpc) builds a spec-conformant gRPC client (and server) on top of `trillium-client`. You write a `.proto`, codegen produces a typed `<Service>Client` wrapping a `Client`, and each RPC shape — unary, server-streaming, client-streaming, bidirectional — gets a call handle that fits it. See [its documentation](https://docs.rs/trillium-grpc) for the full guide.
