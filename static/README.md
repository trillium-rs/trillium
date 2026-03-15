# 📂 trillium-static — static file serving from the filesystem

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-static.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-static
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-static

Serves static files directly from the filesystem. Supports index files, MIME type detection, and
`ETag`-based caching. For embedding files into the binary at compile time, see
[`trillium-static-compiled`](https://docs.rs/trillium-static-compiled).

> **Note:** This crate requires enabling a runtime feature: `smol`, `tokio`, or `async-std`.

## Example

```rust,no_run
use trillium_static::{StaticFileHandler, crate_relative_path};

let handler = StaticFileHandler::new(crate_relative_path!("public"))
    .with_index_file("index.html");
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
