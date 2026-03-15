# 🌊 trillium-async-std — async-std runtime adapter

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-async-std.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-async-std
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-async-std

async-std runtime adapter for Trillium. Provides `run` and `run_async` entry points and a `config()` builder for server configuration. Also provides `ClientConfig` for use with [`trillium-client`](https://docs.rs/trillium-client).

## Example

```rust,no_run
fn main() {
    trillium_async_std::run(|conn: trillium::Conn| async move { conn.ok("hello async-std") });
}
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
