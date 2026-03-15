# 🌺 trillium — a modular async web framework

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium.svg?style=flat-square
[crate]: https://crates.io/crates/trillium
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium

📖 [Guide](https://trillium.rs) · 📑 [Rustdocs](https://docs.trillium.rs)

Trillium is a modular async web framework for Rust. Its key design point: **there is no distinction between middleware and endpoints** — everything is a `Handler`. Handlers compose by nesting in tuples, running left to right and stopping at the first that halts the connection. This makes it easy to mix routing, logging, auth, compression, and any other behavior without special-casing at the framework level.

To build a Trillium app you also need a runtime adapter: [`trillium-tokio`](https://docs.rs/trillium-tokio), [`trillium-smol`](https://docs.rs/trillium-smol), or [`trillium-async-std`](https://docs.rs/trillium-async-std).

## Example

```rust,no_run
use trillium::Conn;

// Any async fn(Conn) -> Conn is a Handler:
async fn hello(conn: Conn) -> Conn {
    conn.ok("hello trillium!")
}

// Handlers compose left-to-right in a tuple.
// The first handler to call conn.halt() stops the chain.
let app = (
    hello,
    // add trillium_logger::logger(), trillium_router::router(), etc. here
);

// run with your chosen runtime adapter:
// trillium_tokio::run(app);
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
