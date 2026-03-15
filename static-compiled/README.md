# 📦 trillium-static-compiled — embedded static file serving

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-static-compiled.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-static-compiled
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-static-compiled

Serves static files embedded directly into the binary at compile time. No runtime feature flag
required. For serving files from the filesystem at runtime, see
[`trillium-static`](https://docs.rs/trillium-static).

## Example

```rust,no_run
use trillium_static_compiled::static_compiled;

let handler = static_compiled!("./public").with_index_file("index.html");
// run with your chosen runtime adapter, e.g.:
// trillium_tokio::run(handler);
```

The `static_compiled!` macro embeds the directory contents at compile time, so the binary is
self-contained — no filesystem access at runtime.

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
