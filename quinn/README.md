# ⚡ trillium-quinn — QUIC transport for HTTP/3

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-quinn.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-quinn
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-quinn

Quinn-backed QUIC transport for Trillium, enabling HTTP/3 alongside any Trillium server adapter. Add `QuicConfig` to your server config and TLS acceptor to serve HTTP/3 and HTTP/1.x on the same port. Requires TLS; default crypto backend is `aws-lc-rs` (or `ring` via feature flag).

## Example

```rust,no_run
use trillium::Conn;
use trillium_quinn::QuicConfig;
use trillium_rustls::RustlsAcceptor;

fn main() {
    let cert = std::fs::read("cert.pem").unwrap();
    let key = std::fs::read("key.pem").unwrap();

    trillium_tokio::config()
        .with_acceptor(RustlsAcceptor::from_single_cert(&cert, &key))
        .with_quic(QuicConfig::from_single_cert(&cert, &key))
        .run(|conn: Conn| async move { conn.ok("http/3 works") });
}
```

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
