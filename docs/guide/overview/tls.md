# TLS

All adapters (except `aws_lambda`) can be combined with a TLS acceptor.

## Rustls

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

## Native TLS

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

`trillium-native-tls` does not currently negotiate ALPN, so HTTP/2 is not available with this
adapter. If you need HTTP/2, use `trillium-rustls` or `trillium-openssl`.

## OpenSSL

[rustdocs](https://docs.trillium.rs/trillium_openssl)

Backed by [`async-openssl`](https://crates.io/crates/async-openssl) and the
[`openssl`](https://crates.io/crates/openssl) crate. Use this when you need OpenSSL specifically
(e.g. FIPS-validated builds, organization-mandated cryptography library) and still want HTTP/2.
Requires a system OpenSSL install at build time, or enable the `vendored` cargo feature to compile
OpenSSL from source.

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-openssl = { path = "../openssl" }
# env_logger = "*"
#
use trillium::Conn;
use trillium_openssl::OpenSslAcceptor;

# const KEY: &[u8] = b"";
# const CERT: &[u8] = b"";

pub fn main() {
    env_logger::init();
    trillium_smol::config()
        .with_acceptor(OpenSslAcceptor::from_single_cert(CERT, KEY))
        .run(|conn: Conn| async move { conn.ok("ok") });
}
```

## Automatic HTTPS via Let's Encrypt

See [`trillium-acme`](https://docs.rs/trillium-acme) for automatic certificate provisioning from ACME providers like Let's Encrypt.

