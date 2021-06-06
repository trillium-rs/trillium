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

```rust,noplaypen
{{#include ../../../smol/examples/smol-with-config.rs}}
```

In addition to accepting the `HOST` and `PORT` configuration from the environment, on cfg(unix) systems, trillium will also pick up a `LISTEN_FD` environment variable for use with [catflap](https://crates.io/crates/catflap)/[systemfd](https://github.com/mitsuhiko/systemfd)

For more documentation on the default values and what configuration can be chained onto config(), see [trillium_server_common::Config](https://docs.trillium.rs/trillium_server_common/struct.config).

###

## TLS / HTTPS

With the exception of aws lambda, which provides its own tls
termination at the load balancer, each of the above servers can be
combined with either rustls or native-tls.

### Rustls:
[rustdocs (main)](https://docs.trillium.rs/trillium_rustls/index.html)

```rust,noplaypen
{{#include ../../../rustls/examples/rustls.rs}}
```

### Native tls:
[rustdocs (main)](https://docs.trillium.rs/trillium_native_tls/index.html)

```rust,noplaypen
{{#include ../../../native-tls/examples/native-tls.rs}}
```

