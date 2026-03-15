# 📦 trillium-static-compiled-macros — proc macro support for trillium-static-compiled

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-static-compiled-macros.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-static-compiled-macros
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-static-compiled-macros

Internal proc macro crate backing [`trillium-static-compiled`](https://docs.rs/trillium-static-compiled). Provides `include_dir!` and `include_entry!` macros that embed directory contents at compile time, with filesystem metadata. This crate is not intended for direct use — depend on `trillium-static-compiled` instead.

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
