# Welcome

Trillium is a modular async Rust web framework. Features such as routing, sessions, compression,
WebSockets, [and many more](./handlers) are published as separate opt-in crates, and the unifying
abstraction is the [`Handler`](https://docs.trillium.rs/trillium/trait.Handler.html) trait.

## Handlers

The simplest handler is an async function or closure that takes a `Conn` and returns it:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
#
fn main() {
    trillium_smol::run(|conn: trillium::Conn| async move {
        conn.ok("hello from trillium!")
    });
}
```

```bash
cargo add trillium trillium-smol
```

Run it and visit `http://localhost:8080`.

A logger, a cookie extractor, an authentication gate, and a JSON endpoint are all `Handler`s. They
compose via tuples, which run left to right:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-logger = { path = "../logger" }
# trillium-cookies = { path = "../cookies" }
#
# fn main() {
# use trillium_logger::Logger;
# use trillium_cookies::CookiesHandler;
# async fn router(conn: trillium::Conn) -> trillium::Conn { conn }
trillium_smol::run((
    Logger::new(),
    CookiesHandler::new(),
    router,
));
# }
```

Each handler receives the `Conn`, does its work, and either passes it along or halts. Halting stops
the chain — subsequent handlers are skipped. A handler signals "I've handled this" by calling
`.halt()`, or a convenience method like `.ok("body")` that halts implicitly. There is no distinction
between middleware and endpoints — they're all handlers.

## Conn

`Conn` carries the HTTP request and response through the handler chain. It also owns the underlying
connection — dropping a `Conn` disconnects the client.

Response building is chainable:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
#
# fn main() {
#     trillium_smol::run(|conn: trillium::Conn| async move {
conn.with_status(202)
    .with_response_header("content-type", "text/plain")
    .with_body("hello")
    .halt()
#     });
# }
```

If a handler returns `Conn` without halting, the response defaults to 404 and the next handler in
the chain runs. This is always valid — it means "I didn't handle this."

`Conn` also carries a type-indexed state set. Handlers use it to pass data down the chain: an auth
handler early in the tuple stores the current user in state, and later handlers retrieve it by type.

## Runtime adapters

`trillium_smol::run` in the examples above is a **runtime adapter** — it binds a port, listens for
TCP connections, and drives the async executor. Adapters are available for smol, tokio, async-std,
and AWS Lambda. TLS and HTTP/3 are also configured at this layer.

See [Runtime Adapters, TLS, and HTTP/3](./overview/runtimes.md) for the full picture.

## Where to go next

- [Handlers in depth](./overview/handlers.md) — the trait, built-in implementations, and the `init`
  lifecycle
- [Conn in depth](./overview/conn.md) — request access, state, and the conn extension pattern
- [Runtime Adapters, TLS, and HTTP/3](./overview/runtimes.md) — runtime selection, configuration,
  TLS, and QUIC
- [Handler Libraries](./handlers.md) — a tour of all official handler crates
