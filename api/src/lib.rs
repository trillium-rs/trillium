/*!
This crate provides handlers for common http api behavior.

Eventually, some of this crate may move into the trillium crate, but
for now it exists separately for ease of iteration. Expect more
breaking changes in this crate then in the trillium crate.

## Formats supported:

Currently, this crate supports *receiving* `application/json` and
`application/x-form-www-urlencoded` by default. To disable
`application/x-form-www-urlencoded` support, use `default-features =
false`.

This crate currently only supports sending json responses, but may
eventually add `Accepts` negotiation and further outbound response
content types.

The [`ApiConnExt`] extension trait and [`ApiHandler`] can be used
independently or in combination.

[`ApiHandler`] provides an easy way to deserialize a single type from
the request body. ApiHandler does not handle serializing responses, so
is best used in conjunction with the [`Json`] handler that this crate
provides.

If [`ApiHandler`] encounters an error of any sort before the
user-provided logic is executed, it will put an [`Error`] into the
conn's state. A default error handler is provided.
*/
#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

mod api_conn_ext;
mod api_handler;
mod body;
mod default_error_handler;
mod error;
mod from_conn;
mod json;
mod state;

pub use api_conn_ext::ApiConnExt;
pub use api_handler::{api, ApiHandler};
pub use body::Body;
pub use error::Error;
pub use from_conn::FromConn;
pub use json::Json;
pub use serde_json::{json, Value};
pub use state::State;
