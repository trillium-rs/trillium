# 🗜️ trillium-compression — response body compression

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-compression.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-compression
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-compression

Transparent response body compression for trillium. Supports zstd, brotli, and gzip (in that
preference order). The algorithm is selected based on the client's `Accept-Encoding` header;
downstream handlers need no modification.

## Example

```rust,no_run
use trillium_compression::Compression;

let app = (
    Compression::new(),
    |conn: trillium::Conn| async move { conn.ok("hello world") },
);
// run with your chosen runtime adapter, e.g.:
// trillium_tokio::run(app);
```

The set of supported algorithms can be restricted if needed:

```rust,no_run
use trillium_compression::{Compression, CompressionAlgorithm};

let app = (
    Compression::new().with_algorithms(&[CompressionAlgorithm::Gzip]),
    |conn: trillium::Conn| async move { conn.ok("hello world") },
);
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
