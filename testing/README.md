# 🧪 trillium-testing — test utilities for Trillium apps

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-testing.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-testing
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-testing

Testing utilities for Trillium applications. Provides assertion macros (`assert_ok!`, `assert_status!`, `assert_response!`, `assert_not_handled!`), a `TestConn` builder for constructing requests, and a synchronous test runner — no real TCP listener needed. Enable one of the `tokio`, `smol`, or `async-std` cargo features to select the async runtime.

## Example

```rust
use trillium::Conn;
use trillium_testing::prelude::*;

async fn handler(conn: Conn) -> Conn {
    conn.ok("hello testing")
}

assert_ok!(get("/").on(&handler), "hello testing");
assert_not_handled!(get("/").on(&()));
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
