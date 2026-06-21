# 📦 trillium-logger-macros — proc macro support for trillium-logger

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-logger-macros.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-logger-macros
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-logger-macros

Internal proc macro crate backing [`trillium-logger`](https://docs.rs/trillium-logger). Provides the
`log_format!` and `client_log_format!` macros for building log formatters from a `format_args!`-style
string. This crate is not intended for direct use — depend on `trillium-logger` instead.

## License

<sup>
Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.
</sup>
