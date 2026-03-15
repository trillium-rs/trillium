# 🔒 trillium-rustls — TLS via rustls

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-rustls.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-rustls
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-rustls

TLS adapter for Trillium using [rustls](https://docs.rs/rustls). Provides `RustlsAcceptor` for TLS-secured servers and `RustlsClientTransport` for TLS-capable clients. The default crypto backend is `aws-lc-rs`; opt into `ring` or `custom-crypto-provider` via cargo features.

## Example

```rust,no_run
use trillium::Conn;
use trillium_rustls::RustlsAcceptor;

fn main() {
    let cert = std::fs::read("cert.pem").unwrap();
    let key = std::fs::read("key.pem").unwrap();
    trillium_smol::config()
        .with_acceptor(RustlsAcceptor::from_single_cert(&cert, &key))
        .run(|conn: Conn| async move { conn.ok("https works") });
}
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
