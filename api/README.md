# 🔌 trillium-api — extractor-based API handler

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-api.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-api
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-api

Extractor-based API layer for Trillium. Wraps an async function into a [`Handler`](https://docs.rs/trillium/latest/trillium/trait.Handler.html): the function receives a `&mut Conn` and a typed value extracted from the request (deserialized body, shared state, route params, etc.) and returns any type that implements `Handler`.

## Example

```rust,no_run
use trillium_api::{api, Json};
use trillium::Conn;

/// No input, JSON response.
async fn hello(_conn: &mut Conn, _: ()) -> Json<&'static str> {
    Json("hello, world")
}

/// Deserialize a JSON body and echo it back.
async fn echo(
    _conn: &mut Conn,
    Json(body): Json<trillium_api::Value>,
) -> Json<trillium_api::Value> {
    Json(body)
}

let app = (api(hello), api(echo));
// run with your chosen runtime adapter, e.g.:
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
