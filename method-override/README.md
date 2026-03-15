# 🔧 trillium-method-override — HTML form method override

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-method-override.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-method-override
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-method-override

Allows HTML forms and other HTTP clients that can only send `GET` and `POST` requests to use
`PUT`, `PATCH`, and `DELETE` by adding a `_method` query parameter. Downstream handlers see the
overridden method, not `POST`.

## Example

```rust,no_run
use trillium_method_override::MethodOverride;
use trillium_router::Router;

let app = (
    MethodOverride::new(),
    Router::new()
        .delete("/posts/:id", |conn: trillium::Conn| async move {
            conn.ok("deleted")
        }),
);
// A POST to /posts/1?_method=DELETE is routed as DELETE /posts/1
// run with your chosen runtime adapter, e.g.:
// trillium_tokio::run(app);
```

The default query parameter name is `_method` and the allowed override methods are `PUT`, `PATCH`,
and `DELETE`. Both can be customized:

```rust,no_run
use trillium_method_override::MethodOverride;

let handler = MethodOverride::new()
    .with_param_name("_http_method")
    .with_allowed_methods(["put", "patch", "delete"]);
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
