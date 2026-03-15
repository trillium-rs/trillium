# 🔀 trillium-router — HTTP request router for trillium

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-router.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-router
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-router

`trillium-router` provides URL-based request routing for trillium applications. Routes are matched
in registration order and support named parameters (`:name`), wildcards (`*`), and any HTTP method.
Route resolution is handled by [routefinder](https://github.com/jbr/routefinder).

## Example

```rust
use trillium::{Conn, conn_unwrap};
use trillium_router::{Router, RouterConnExt};

let router = Router::new()
    .get("/", |conn: Conn| async move {
        conn.ok("you have reached the index")
    })
    .get("/pages/:page_name", |conn: Conn| async move {
        let page_name = conn_unwrap!(conn.param("page_name"), conn);
        conn.ok(format!("page: {page_name}"))
    })
    .get("/files/*", |conn: Conn| async move {
        let path = conn_unwrap!(conn.wildcard(), conn);
        conn.ok(format!("file: {path}"))
    });

use trillium_testing::prelude::*;
assert_ok!(get("/").on(&router), "you have reached the index");
assert_ok!(get("/pages/trillium").on(&router), "page: trillium");
assert_ok!(get("/files/images/logo.png").on(&router), "file: images/logo.png");
assert_not_handled!(get("/unknown/route").on(&router));
```

## Options handling

By default, the router responds to `OPTIONS` requests with the list of HTTP methods supported at
that route. Sending `OPTIONS *` returns all methods the router supports across all routes.

This behavior is superseded by an explicit `OPTIONS` handler or an `any` handler, and can be
disabled with [`Router::without_options_handling`][docs].

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
