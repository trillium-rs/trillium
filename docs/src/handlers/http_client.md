# HTTP Client

[rustdocs](https://docs.trillium.rs/trillium_client)

`trillium-client` is a full HTTP client that mirrors the server-side `Conn` design. It supports HTTP/1.1, HTTPS via rustls or native-tls, HTTP/3 via QUIC, connection pooling, and WebSocket upgrades.

The client is runtime-agnostic and uses the same connector pattern as the server adapters.

## Basic usage

```rust,noplaypen
use trillium_client::Client;
use trillium_smol::ClientConfig;

async fn fetch() {
    let client = Client::new(ClientConfig::default());

    let body = client
        .get("http://example.com/")
        .await
        .unwrap()
        .success()
        .unwrap()
        .response_body()
        .await
        .unwrap();

    println!("{body}");
}
```

`ClientConfig` comes from your chosen runtime crate. The `success()` method returns an error if the status is not 2xx.

## HTTPS

Wrap `ClientConfig` with a TLS config:

```rust,noplaypen
use trillium_client::Client;
use trillium_rustls::RustlsConfig;
use trillium_smol::ClientConfig;

let client = Client::new(RustlsConfig::<ClientConfig>::default());
let conn = client.get("https://example.com/").await.unwrap();
```

`trillium-native-tls` can be used instead of `trillium-rustls` with the same pattern.

## HTTP/3

The client upgrades to HTTP/3 automatically when a server advertises support via `Alt-Svc`. The first request to a host uses HTTP/1.1; if the response includes `Alt-Svc: h3=...`, subsequent requests to that host use HTTP/3.

```rust,noplaypen
use trillium_client::Client;
use trillium_quinn::ClientQuicConfig;
use trillium_rustls::RustlsConfig;
use trillium_tokio::ClientConfig;

let client = Client::new_with_quic(
    RustlsConfig::<ClientConfig>::default(),
    ClientQuicConfig::with_webpki_roots(),
);

// Request 1: HTTP/1.1 (no Alt-Svc cached)
// Request 2+: HTTP/3 if the server advertised it
for _ in 0..3 {
    let conn = client.get("https://cloudflare.com/").await.unwrap();
    println!("{:?}", conn.http_version());
}
```

## Making requests

The client has methods for each HTTP verb. Each returns a `Conn` that you can configure before sending:

```rust,noplaypen
let conn = client
    .post("https://api.example.com/items")
    .with_request_header("content-type", "application/json")
    .with_request_body(r#"{"name":"widget"}"#)
    .await
    .unwrap();

println!("status: {}", conn.status().unwrap());
```

## Connection pooling

Connections are pooled and reused automatically across requests to the same host. The pool handles HTTP/1.1 and HTTP/3 connections separately.

## WebSocket client

With the `websockets` feature, the client can upgrade a connection to WebSocket:

```rust,noplaypen
let ws_conn = client
    .get("wss://example.com/ws")
    .await
    .unwrap()
    .into_websocket()
    .await
    .unwrap();
```

The resulting `WebSocketConn` exposes the same send/receive interface as the server-side WebSocket handler.
