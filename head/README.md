# 📋 trillium-head — HTTP HEAD method support

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-head.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-head
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-head

Transparent `HEAD` request handling for trillium. Incoming `HEAD` requests are rewritten to `GET`
so downstream handlers work without modification; before the response is sent, the body is stripped
and a `Content-Length` header is added. Place this early in your handler chain.

## Example

```rust,no_run
use trillium::Conn;
use trillium_head::Head;

let app = (
    Head::new(),
    |conn: Conn| async move { conn.ok("hello world") },
);
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
