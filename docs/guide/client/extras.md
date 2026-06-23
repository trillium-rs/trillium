# WebSockets, WebTransport, SSE, and JSON

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

## Server-Sent Events

[Server-Sent Events](https://html.spec.whatwg.org/multipage/server-sent-events.html) is a one-way stream of text events over an ordinary HTTP response — the server holds the response open and writes `data:`/`event:`/`id:` lines as things happen. Unlike WebSockets and WebTransport, there is no protocol upgrade; it works the same over HTTP/1.1, HTTP/2, and HTTP/3.

With the `sse` feature, `Conn::into_sse()` sends the request, checks for a success status and a `text/event-stream` content-type, and hands back an `EventStream` — a `Stream` of `Event`s. Note that `into_sse()` *is* the execution, so build the conn but don't await it yourself first.

```rust
# [dependencies]
# trillium-smol = { path = "../smol" }
# trillium-client = { path = "../client", features = ["sse"] }
# trillium-testing = { path = "../testing" }
# futures-lite = "2.6.1"
#
# fn main() { trillium_testing::block_on(async {
use futures_lite::StreamExt;
use trillium_client::Client;
use trillium_smol::ClientConfig;

# let client = Client::new(ClientConfig::default());
let mut events = client
    .get("https://example.com/events")
    .into_sse()
    .await
    .unwrap();

while let Some(event) = events.next().await {
    let event = event.unwrap();
    println!("{}", event.data());
}
# }); }
```

Each `Event` exposes `data()`, `event_type()` (`None` for the default `message` type), `id()`, and `retry()`. The stream yields `Result` items because reading the underlying connection can fail mid-stream; it ends when the connection closes. This is a single-response stream — it does not reconnect on its own. If you need the browser `EventSource`'s automatic reconnection (re-issuing the request with `Last-Event-ID`), build that on top, or drive the whole request through a retrying [`ClientHandler`](./middleware.md). On failure, `into_sse()` returns an `SseError` you can dereference as a `Conn` to inspect the response — for example, to read an error body returned with a non-2xx status.

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
