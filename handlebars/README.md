# 🖋️ trillium-handlebars — Handlebars template rendering

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-handlebars.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-handlebars
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-handlebars

[Handlebars](https://handlebarsjs.com/) template rendering for trillium. `HandlebarsHandler`
loads templates from the filesystem at startup; handlers can then assign variables and render
templates using the [`HandlebarsConnExt`][docs] extension trait.

## Example

```rust,no_run
use trillium_handlebars::{HandlebarsConnExt, HandlebarsHandler};

let handler = (
    HandlebarsHandler::new("templates/**/*.hbs"),
    |mut conn: trillium::Conn| async move {
        conn.assign("name", "trillium").render("hello.hbs")
    },
);
// run with your chosen runtime adapter, e.g.:
// trillium_tokio::run(handler);
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
