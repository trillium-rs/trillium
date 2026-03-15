# 🍪 trillium-cookies — cookie parsing and management

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-cookies.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-cookies
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-cookies

Cookie parsing and jar management for trillium. Place `CookiesHandler` early in your handler
chain; downstream handlers can then read and write cookies through the [`CookiesConnExt`][docs]
extension trait. This crate re-exports the [`cookie`][docs] crate for constructing `Cookie` values.

## Example

```rust,no_run
use trillium::Conn;
use trillium_cookies::{CookiesConnExt, CookiesHandler};

let app = (
    CookiesHandler::new(),
    |conn: Conn| async move {
        let count: u32 = conn
            .cookies()
            .get("visits")
            .and_then(|c| c.value().parse().ok())
            .unwrap_or(0)
            + 1;
        conn.with_cookie(("visits", count.to_string()))
            .ok(format!("you have visited {count} times"))
    },
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
