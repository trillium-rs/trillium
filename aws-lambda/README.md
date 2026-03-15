# ☁️ trillium-aws-lambda — AWS Lambda runtime adapter

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-aws-lambda.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-aws-lambda
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-aws-lambda

Run trillium handlers on [AWS Lambda](https://aws.amazon.com/lambda/) behind an Application Load
Balancer. The [`LambdaConnExt`][docs] trait provides access to the Lambda context from within
handlers.

## Example

```rust,no_run
use trillium::Conn;

trillium_aws_lambda::run(|conn: Conn| async move {
    conn.ok("hello from lambda")
});
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
