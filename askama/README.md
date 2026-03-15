# ✏️ trillium-askama — compile-time Jinja-like templates

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-askama.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-askama
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-askama

[Askama](https://github.com/djc/askama) template rendering for trillium. Templates are
type-checked and compiled into Rust code at build time. Derive `Template` on a struct and call
`conn.render(template)` from any handler.

## Example

```rust,no_run
use trillium::Conn;
use trillium_askama::{AskamaConnExt, Template};

#[derive(Template)]
#[template(path = "hello.html")]
struct HelloTemplate<'a> {
    name: &'a str,
}

async fn handler(conn: Conn) -> Conn {
    conn.render(HelloTemplate { name: "trillium" })
}
// run with your chosen runtime adapter, e.g.:
// trillium_tokio::run(handler);
```

Template files live under the `templates/` directory in your crate root by default. See the
[askama docs](https://djc.github.io/askama/) for the full template syntax.

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
