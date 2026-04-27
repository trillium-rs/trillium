# trillium-openssl

OpenSSL adapter for [trillium.rs](https://trillium.rs), backed by
[`async-openssl`](https://crates.io/crates/async-openssl) and the
[`openssl`](https://crates.io/crates/openssl) crate.

This crate provides an alternative to [`trillium-rustls`](https://crates.io/crates/trillium-rustls)
and [`trillium-native-tls`](https://crates.io/crates/trillium-native-tls) for applications that
prefer OpenSSL. Unlike `trillium-native-tls`, this crate negotiates ALPN, so HTTP/2 works on both
client and server.

```rust,no_run
use trillium::Conn;
use trillium_openssl::OpenSslAcceptor;

const KEY: &[u8] = include_bytes!("../examples/key.pem");
const CERT: &[u8] = include_bytes!("../examples/cert.pem");

trillium_smol::config()
    .with_acceptor(OpenSslAcceptor::from_single_cert(CERT, KEY))
    .run(|conn: Conn| async move { conn.ok("ok") });
```

## OpenSSL system dependency

This crate depends on a system OpenSSL install at build time, the same as the underlying `openssl`
crate. To build with a vendored copy of OpenSSL instead, enable the `vendored` cargo feature, which
forwards to `openssl/vendored`.
