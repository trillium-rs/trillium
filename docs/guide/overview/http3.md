# HTTP/3 and QUIC

HTTP/3 is the third major revision of the HTTP protocol. Instead of TCP, it's built on QUIC, a UDP-based transport that was designed with multiplexed HTTP in mind. This gives HTTP/3 several advantages over HTTP/1.x:

- **Faster connection setup** — QUIC combines the transport and TLS handshakes, reducing round trips from three (TCP + TLS 1.3) to one.
- **No head-of-line blocking** — HTTP/3 sends each request on an independent QUIC stream. A stalled or slow response doesn't delay any other response on the same connection.
- **Better performance on lossy networks** — particularly relevant for mobile clients where packet loss is common.

## Adding HTTP/3 to a server

The `trillium-quinn` crate adds HTTP/3 support to any Trillium server. It runs a QUIC endpoint alongside the existing TCP listener. Protocol selection happens automatically: browsers use ALPN during the TLS handshake to negotiate HTTP/3, and the server advertises support via `Alt-Svc` headers so clients know to use QUIC on future connections.

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-tokio = { path = "../tokio" }
# trillium-quinn = { path = "../quinn" }
# trillium-rustls = { path = "../rustls" }
#
use trillium::Conn;
use trillium_quinn::QuicConfig;
use trillium_rustls::RustlsAcceptor;

fn main() {
#     let cert = b"";
#     let key = b"";

    trillium_tokio::config()
        .with_acceptor(RustlsAcceptor::from_single_cert(cert, key))
        .with_quic(QuicConfig::from_single_cert(cert, key))
        .run(|conn: Conn| async move { conn.ok("hello!") });
}
```

As with HTTP/2, the same handler receives `Conn` objects regardless of whether they arrived over HTTP/1.x, HTTP/2, or HTTP/3. No changes to application logic are required.

> ℹ️ TLS is required for HTTP/3. The `RustlsAcceptor` and `QuicConfig` can be initialized from the same certificate and key files.

## Crypto providers

`trillium-quinn` uses `aws-lc-rs` as its crypto backend by default. To use `ring` instead, enable the `ring` feature and disable `aws-lc-rs`.

## HTTP/3 client

The `trillium-client` HTTP client can upgrade to HTTP/3 automatically when a server advertises support via `Alt-Svc`. See the [HTTP Client](../client/overview.md) page for details.

## WebTransport

WebTransport is a browser API built on top of HTTP/3 that provides multiplexed streams and datagrams for real-time communication. It requires HTTP/3 to be configured. See [WebTransport](../handlers/webtransport.md) for details.
