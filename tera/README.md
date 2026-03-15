# 🌿 trillium-tera — Tera template rendering

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-tera.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-tera
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-tera

[Tera](https://keats.github.io/tera/) template rendering for trillium. Tera uses a
Jinja2/Django-inspired syntax. Initialize a `Tera` instance with your templates, pass it to
`TeraHandler`, then assign variables and render from any downstream handler using
[`TeraConnExt`][docs].

## Example

```rust,no_run
use trillium::Conn;
use trillium_tera::{Tera, TeraConnExt, TeraHandler};

let mut tera = Tera::default();
tera.add_raw_template("hello.html", "hello {{ name }}!").unwrap();

let handler = (
    TeraHandler::new(tera),
    |conn: Conn| async move { conn.assign("name", "trillium").render("hello.html") },
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
