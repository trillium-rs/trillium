# Runtime Adapters and TLS

## Runtime Adapters

Let's talk a little more about that `trillium_smol::run` line we've
been writing. Trillium itself is built on `futures` (`futures-lite`,
specifically). In order to run it, it needs an adapter to an async
runtime. There there are four of these
currently:

* [`trillium_smol`](https://docs.trillium.rs/trillium_smol)
* [`trillium_async_std`](https://docs.trillium.rs/trillium_async_std)
* [`trillium_tokio`](https://docs.trillium.rs/trillium_tokio)
* [`trillium_aws_lambda`](https://docs.trillium.rs/trillium_aws_lambda)

Although we've been using the smol adapter in these docs thus far, you
should use whichever runtime you prefer. If you expect to have a
dependency on async-std or tokio anyway, you might as well use the
adapter for that runtime. If you're new to async rust or don't have an
opinion, I recommend starting with trillium_smol. It is easy to switch
trillium between runtimes at any point.

# 12-Factor by default, but overridable

Trillium seeks to abide by a [12 factor](https://12factor.net/config) approach to configuration, accepting configuration from the environment wherever possible. The number of configuration points that can be customized through environment variables will likely increase over time.

To run trillium on a different host or port, either provide a `HOST`
and/or `PORT` environment variables, or compile the specific values
into the application as follows:

```rust
pub fn main() {
    trillium_smol::config()
        .with_port(1337)
        .with_host("127.0.0.1")
        .run(|conn: trillium::Conn| async move { conn.ok("hello world") })
}
```

In addition to accepting the `HOST` and `PORT` configuration from the environment, on cfg(unix) systems, trillium will also pick up a `LISTEN_FD` environment variable for use with [catflap](https://crates.io/crates/catflap)/[systemfd](https://github.com/mitsuhiko/systemfd). On `cfg(unix)` systems, if the `HOST` begins with `.`, `/`, or `~`, it is interpreted as a path and bound as a unix domain socket.

For more documentation on the default values and what configuration can be chained onto config(), see [trillium_server_common::Config](https://docs.trillium.rs/trillium_server_common/struct.config).

###

## TLS / HTTPS

With the exception of aws lambda, which provides its own tls
termination at the load balancer, each of the above servers can be
combined with either rustls or native-tls, or with `trillium-acme` to register
a certificate automatically with an ACME certificate provider like Let's
Encrypt.

### Rustls:
[rustdocs (main)](https://docs.trillium.rs/trillium_rustls/index.html)

```rust
use trillium::Conn;
use trillium_rustls::RustlsAcceptor;

const KEY: &[u8] = include_bytes!("./key.pem");
const CERT: &[u8] = include_bytes!("./cert.pem");

pub fn main() {
    env_logger::init();
    trillium_smol::config()
        .with_acceptor(RustlsAcceptor::from_single_cert(CERT, KEY))
        .run(|conn: Conn| async move { conn.ok("ok") });
}
```

### Native tls:
[rustdocs (main)](https://docs.trillium.rs/trillium_native_tls/index.html)

```rust
use trillium::Conn;
use trillium_native_tls::NativeTlsAcceptor;

pub fn main() {
    env_logger::init();
    let acceptor = NativeTlsAcceptor::from_pkcs12(include_bytes!("./identity.p12"), "changeit");
    trillium_smol::config()
        .with_acceptor(acceptor)
        .run(|conn: Conn| async move { conn.ok("ok") });
}
```

### Automatic HTTPS via Let's Encrypt:
See the [`trillium-acme` documentation](https://docs.rs/trillium-acme/latest/trillium_acme/) for examples.
