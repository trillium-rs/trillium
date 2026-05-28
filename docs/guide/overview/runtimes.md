# Runtime adapters

Trillium is built on `futures-lite` and is async-runtime-agnostic. To run a server, pick one adapter crate:

- [`trillium_smol`](https://docs.trillium.rs/trillium_smol) — built on `smol`. Lightweight and fast. A good default if you don't have a runtime preference.
- [`trillium_tokio`](https://docs.trillium.rs/trillium_tokio) — built on `tokio`. Use this if your application already depends on tokio.
- [`trillium_async_std`](https://docs.trillium.rs/trillium_async_std) — built on `async-std`.
- [`trillium_aws_lambda`](https://docs.trillium.rs/trillium_aws_lambda) — runs on AWS Lambda. TLS and HTTP/3 are handled by the load balancer; no TLS configuration is needed.

All adapters expose the same `config()` builder and `run()` function, so switching runtimes is a one-line change.

From there, the rest of serving a request is the same whichever runtime you pick:

- [Listeners](./listeners.md) — where and how the server binds, including multiple listeners on one server.
- [Graceful shutdown](./graceful-shutdown.md) — draining cleanly, and running several servers in one process.
- [TLS](./tls.md) — serving over HTTPS with rustls, native-tls, or OpenSSL.
- [HTTP/2](./http2.md) and [HTTP/3](./http3.md) — the newer HTTP versions, which trillium speaks transparently.
