# 🔐 trillium-basic-auth — HTTP Basic authentication

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-basic-auth.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-basic-auth
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-basic-auth

HTTP Basic authentication for trillium. Handlers placed after `BasicAuth` in the chain only run
for authenticated requests; unauthenticated requests receive a `401 Unauthorized` with a
`WWW-Authenticate` challenge header.

## Example

```rust,no_run
use trillium_basic_auth::BasicAuth;

let app = (
    BasicAuth::new("trillium", "hunter2").with_realm("my app"),
    |conn: trillium::Conn| async move { conn.ok("authenticated!") },
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
