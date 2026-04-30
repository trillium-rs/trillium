# Runtime Adapters, TLS, HTTP/2, and HTTP/3

## Runtime adapters

Trillium is built on `futures-lite` and is async-runtime-agnostic. To run a server, pick one adapter crate:

- [`trillium_smol`](https://docs.trillium.rs/trillium_smol) — built on `smol`. Lightweight and fast. A good default if you don't have a runtime preference.
- [`trillium_tokio`](https://docs.trillium.rs/trillium_tokio) — built on `tokio`. Use this if your application already depends on tokio.
- [`trillium_async_std`](https://docs.trillium.rs/trillium_async_std) — built on `async-std`.
- [`trillium_aws_lambda`](https://docs.trillium.rs/trillium_aws_lambda) — runs on AWS Lambda. TLS and H3 are handled by the load balancer; no TLS configuration is needed.

All adapters expose the same `config()` builder and `run()` function, so switching runtimes is a one-line change.

## Server configuration

By default, Trillium reads `HOST` and `PORT` from the environment. This follows the [12-factor app](https://12factor.net/config) convention.

On Unix systems:
- If `HOST` begins with `.`, `/`, or `~`, it's treated as a filesystem path and bound as a Unix domain socket.
- A `LISTEN_FD` environment variable enables socket activation via [catflap](https://crates.io/crates/catflap) or [systemfd](https://github.com/mitsuhiko/systemfd).

To compile specific host/port values in:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
#
pub fn main() {
    trillium_smol::config()
        .with_port(1337)
        .with_host("127.0.0.1")
        .run(|conn: trillium::Conn| async move { conn.ok("hello world") })
}
```

See [trillium_server_common::Config](https://docs.trillium.rs/trillium_server_common/struct.config) for the full list of configuration options.

## TLS

All adapters (except `aws_lambda`) can be combined with a TLS acceptor.

### Rustls

[rustdocs](https://docs.trillium.rs/trillium_rustls)

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-rustls = { path = "../rustls" }
# env_logger = "*"
#
use trillium::Conn;
use trillium_rustls::RustlsAcceptor;

# const KEY: &[u8] = b"";
# const CERT: &[u8] = b"";

pub fn main() {
    env_logger::init();
    trillium_smol::config()
        .with_acceptor(RustlsAcceptor::from_single_cert(CERT, KEY))
        .run(|conn: Conn| async move { conn.ok("ok") });
}
```

### Native TLS

[rustdocs](https://docs.trillium.rs/trillium_native_tls)

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-native-tls = { path = "../native-tls" }
# env_logger = "*"
#
use trillium::Conn;
use trillium_native_tls::NativeTlsAcceptor;

pub fn main() {
    env_logger::init();
#     let acceptor = NativeTlsAcceptor::from_pkcs12(b"", "changeit");
    trillium_smol::config()
        .with_acceptor(acceptor)
        .run(|conn: Conn| async move { conn.ok("ok") });
}
```

### Automatic HTTPS via Let's Encrypt

See [`trillium-acme`](https://docs.rs/trillium-acme) for automatic certificate provisioning from ACME providers like Let's Encrypt.

## HTTP/2

HTTP/2 multiplexes many requests over a single TCP connection, eliminating the head-of-line blocking and connection-count overhead of HTTP/1.1 pipelining. Trillium speaks HTTP/2 transparently: handlers receive the same `Conn` regardless of whether the request arrived over h1 or h2.

### Over TLS

When `trillium-rustls` is configured, trillium advertises `h2, http/1.1` in ALPN by default. Clients that select `h2` during the TLS handshake get HTTP/2; clients that select `http/1.1` (or that don't speak ALPN) get HTTP/1.1. No additional configuration is required.

`trillium-native-tls` doesn't currently expose ALPN, so ALPN-driven h2 negotiation isn't available there. Clients that send the HTTP/2 preface as the first bytes after the TLS handshake still reach h2 via the prior-knowledge path (see below); clients that don't get HTTP/1.1.

To opt out — for example, when fronting a backend that doesn't speak h2 — drop `h2` from the advertised list:

```rust
# [dependencies]
# trillium-rustls = { path = "../rustls" }
#
# const CERT: &[u8] = b"";
# const KEY: &[u8] = b"";
use trillium_rustls::RustlsAcceptor;

# fn main() {
let acceptor = RustlsAcceptor::from_single_cert_no_h2(CERT, KEY);
# let _ = acceptor;
# }
```

### Prior knowledge

A client may also reach HTTP/2 by sending the HTTP/2 connection preface as the first 24 bytes — over cleartext TCP (h2c) or after a TLS handshake on a connector that doesn't expose ALPN. Trillium peeks at the leading bytes and dispatches to the h2 driver when it sees the preface; otherwise the connection is handled as HTTP/1.x. There is no separate listener and no configuration switch.

> ℹ️ The `Upgrade: h2c` mechanism (RFC 7540 §3.2, removed in RFC 9113) is **not** supported. Use TLS+ALPN or prior knowledge.

### Tuning

HTTP/2-specific knobs (max concurrent streams, flow-control window sizes, max frame size, HPACK table capacity, extended-CONNECT for WebSockets-over-h2) live on `HttpConfig`. See the [`HttpConfig` rustdocs](https://docs.trillium.rs/trillium_http/struct.HttpConfig.html) for the full list.

## HTTP/3 and QUIC

HTTP/3 is the third major revision of the HTTP protocol. Instead of TCP, it's built on QUIC, a UDP-based transport that was designed with multiplexed HTTP in mind. This gives HTTP/3 several advantages over HTTP/1.x:

- **Faster connection setup** — QUIC combines the transport and TLS handshakes, reducing round trips from three (TCP + TLS 1.3) to one.
- **No head-of-line blocking** — HTTP/3 sends each request on an independent QUIC stream. A stalled or slow response doesn't delay any other response on the same connection.
- **Better performance on lossy networks** — particularly relevant for mobile clients where packet loss is common.

### Adding HTTP/3 to a server

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

### Crypto providers

`trillium-quinn` uses `aws-lc-rs` as its crypto backend by default. To use `ring` instead, enable the `ring` feature and disable `aws-lc-rs`.

### HTTP/3 client

The `trillium-client` HTTP client can upgrade to HTTP/3 automatically when a server advertises support via `Alt-Svc`. See the [HTTP Client](../handlers/http_client.md) page for details.

### WebTransport

WebTransport is a browser API built on top of HTTP/3 that provides multiplexed streams and datagrams for real-time communication. It requires HTTP/3 to be configured. See [WebTransport](../handlers/webtransport.md) for details.
