# 💾 trillium-caching-headers — ETag and Last-Modified support

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-caching-headers.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-caching-headers
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-caching-headers

Adds `ETag`/`If-None-Match` and `Last-Modified`/`If-Modified-Since` support to trillium
applications. When a conditional request matches, the handler responds with `304 Not Modified`
instead of sending the full response body, reducing bandwidth and improving perceived performance.

Use `CachingHeaders` (the combined handler) unless you specifically need only one of the two
behaviors.

## Example

```rust,no_run
use trillium_caching_headers::{CachingHeaders, CachingHeadersExt};

let app = (
    CachingHeaders::new(),
    |conn: trillium::Conn| async move {
        conn.with_etag("my-etag-value")
            .ok("hello world")
    },
);
// run with your chosen runtime adapter, e.g.:
// trillium_tokio::run(app);
```

Response headers can also be set via [`CachingHeadersExt`][docs] on `trillium::Headers` directly,
and [`Cache-Control`][docs] headers can be constructed with `CacheControlHeader`.

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
