# HTTP/2

HTTP/2 multiplexes many requests over a single TCP connection, eliminating the head-of-line blocking and connection-count overhead of HTTP/1.1 pipelining. Trillium speaks HTTP/2 transparently: handlers receive the same `Conn` regardless of whether the request arrived over h1 or h2.

## Over TLS

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

## Prior knowledge

A client may also reach HTTP/2 by sending the HTTP/2 connection preface as the first 24 bytes — over cleartext TCP (h2c) or after a TLS handshake on a connector that doesn't expose ALPN. Trillium peeks at the leading bytes and dispatches to the h2 driver when it sees the preface; otherwise the connection is handled as HTTP/1.x. There is no separate listener and no configuration switch.

> ℹ️ The `Upgrade: h2c` mechanism (RFC 7540 §3.2, removed in RFC 9113) is **not** supported. Use TLS+ALPN or prior knowledge.

## Tuning

HTTP/2-specific knobs (max concurrent streams, flow-control window sizes, max frame size, HPACK table capacity, extended-CONNECT for WebSockets-over-h2) live on `HttpConfig`. See the [`HttpConfig` rustdocs](https://docs.trillium.rs/trillium_http/struct.HttpConfig.html) for the full list.

