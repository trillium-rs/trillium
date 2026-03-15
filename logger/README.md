# 📝 trillium-logger — request/response logging

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-logger.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-logger
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-logger

Structured request/response logging for trillium. The default formatter (`dev_formatter`) produces
concise colorized output suitable for development. Built-in `apache_common` and `apache_combined`
formatters are available for production use, and custom formatters can be composed from the
components in the [`formatters`][docs] module.

## Example

```rust,no_run
use trillium_logger::{Logger, apache_combined, formatters};

// default dev formatter — colorized, concise
let app = (Logger::new(), "hello");

// apache combined log format
let app = (
    Logger::new().with_formatter(apache_combined("-", "-")),
    "hello",
);

// compose a custom format from components
let app = (
    Logger::new().with_formatter((
        formatters::method,
        " ",
        formatters::url,
        " -> ",
        formatters::status,
    )),
    "hello",
);
// run with your chosen runtime adapter, e.g.:
// trillium_tokio::run(app);
```

Log output can be directed to stdout (default), a `log` crate backend, or any custom target
implementing [`Targetable`][docs].

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
