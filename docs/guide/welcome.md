# Welcome

Trillium is a modular async Rust web framework. It runs on stable Rust and supports HTTP/1.x and HTTP/3 over QUIC.

The simplest Trillium server:

```rust
fn main() {
    trillium_smol::run(|conn: trillium::Conn| async move {
        conn.ok("hello from trillium!")
    });
}
```

Add it to a project:

```bash
cargo add trillium trillium-smol
```

Visit `http://localhost:8080` and you'll see the response. No configuration needed.

## What's included

Trillium is published as a collection of small, independent crates. The core handles the `Conn` type, the `Handler` trait, and HTTP parsing. Everything else — routing, sessions, websockets, templates — is opt-in and lives in a separate crate. You only compile what you use.

Official crates in this repository:

- **Router** — pattern-based routing with named params and wildcards
- **API layer** — extractor-based handlers with JSON serialization, similar to axum's approach
- **Logger** — configurable HTTP request logging
- **Cookies and sessions** — signed cookies and pluggable session stores
- **Static file serving** — from disk, or baked into the binary at compile time
- **Compression** — gzip, brotli, and zstd based on `Accept-Encoding`
- **WebSockets** — upgrade connections to WebSocket, with access to the original request context
- **Server-sent events** — lightweight server-to-client event streaming
- **Channels** — Phoenix-style topic-based pub/sub over WebSocket
- **WebTransport** — bidirectional streams and datagrams over HTTP/3
- **HTTP client** — a full HTTP client with connection pooling and HTTP/3 support
- **Reverse proxy** — forward requests to upstream servers
- **Template engines** — integrations for Askama, Tera, and Handlebars
- **TLS** — rustls or native-tls, plus automatic certificate provisioning via ACME
- **HTTP/3 over QUIC** — add H3 to any server with `trillium-quinn`

## Where to go next

- [Architectural Overview](./architecture.md) — understand how Trillium works
- [A tour of handler libraries](./handlers.md) — all the official crates with descriptions
- [docs.trillium.rs](https://docs.trillium.rs) — rustdocs for all official crates
- [github.com/trillium-rs/trillium](https://github.com/trillium-rs/trillium) — source code
