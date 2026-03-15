# 🔐 trillium-native-tls — TLS via native-tls

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-native-tls.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-native-tls
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-native-tls

TLS adapter for Trillium using [native-tls](https://docs.rs/native-tls), which delegates to the platform's built-in TLS implementation (SChannel on Windows, Secure Transport on macOS, OpenSSL on Linux). Provides `NativeTlsAcceptor` for servers and `NativeTlsClientTransport` for clients.

## Example

```rust,no_run
use trillium::Conn;
use trillium_native_tls::NativeTlsAcceptor;

fn main() {
    let identity = std::fs::read("identity.p12").unwrap();
    let acceptor = NativeTlsAcceptor::from_pkcs12(&identity, "password");
    trillium_smol::config()
        .with_acceptor(acceptor)
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
