# 🏗️ trillium-server-common — shared server infrastructure

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-server-common.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-server-common
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-server-common

Shared infrastructure crate for Trillium runtime adapters. Provides the `Server` trait, `Config` builder, `Connector` trait for the HTTP client, `Acceptor` for TLS, `Runtime` abstraction, and QUIC/H3 transport primitives. Trillium applications should depend on a specific runtime adapter ([`trillium-tokio`](https://docs.rs/trillium-tokio), [`trillium-smol`](https://docs.rs/trillium-smol), or [`trillium-async-std`](https://docs.rs/trillium-async-std)) rather than this crate directly.

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
