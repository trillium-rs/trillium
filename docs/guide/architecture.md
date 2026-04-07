# Architectural Overview

## Everything is a Handler

The central design decision in Trillium is that **there is no distinction between middleware and endpoints**. A request logger, a cookie extractor, an authentication gate, and a JSON endpoint are all `Handler`s. They compose the same way and obey the same rules.

Handlers compose via tuples, which run left to right:

```rust
# [dependencies]
# trillium-smol = { path = "../smol" }
# trillium-logger = { path = "../logger" }
# trillium-cookies = { path = "../cookies" }
# trillium-router = { path = "../router" }
# trillium = { path = "../trillium" }
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

Each handler receives the `Conn`, does its work, and either passes it along or halts it. Halting stops the chain — subsequent handlers are skipped. This is how endpoints signal that they've handled a request: calling `.halt()` or a convenience method like `.ok("body")` which halts implicitly.

> 🔌 Readers familiar with Elixir's Plug will recognize this as pipelines. The term "halt" is [directly borrowed from Plug](https://hexdocs.pm/plug/Plug.Conn.html#halt/1).

## Only compile what you need

The core `trillium` crate depends only on `futures-lite` and a small set of lightweight crates. Runtime dependencies like `tokio` or `async-std` only enter the build through the adapter crate you explicitly choose. If you don't need a router, the router crate is never compiled.

This containment extends to dependency updates: updating `trillium-sessions` can't introduce a conflict in `trillium-router`, because they are entirely separate crates with independent dependency trees.

## Substitutability

Every component is designed to be replaceable. The core `trillium` crate defines the `Handler` trait and `Conn` type — everything else is optional. Alternative implementations can plug into the same interface without forking anything. An application built against one router can be nested inside an application using a different one, as long as both depend on a compatible version of `trillium`.

## The transport layer

Trillium uses a `Box<dyn Transport>` abstraction so that `Conn` is not generic over transport. TCP, TLS (via rustls or native-tls), and QUIC (for HTTP/3) all implement the same transport trait. Application code operates on `Conn` and never needs to know which transport is in use — a handler processing an HTTP/1.1 connection and the same handler processing an HTTP/3 connection look identical.
