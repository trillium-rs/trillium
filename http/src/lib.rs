#![forbid(unsafe_code)]
#![deny(missing_debug_implementations, nonstandard_style, rust_2018_idioms)]
#![warn(missing_doc_code_examples)]
#![cfg_attr(test, deny(warnings))]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::len_zero)]
#![allow(clippy::match_bool)]
#![allow(clippy::unreadable_literal)]
#![allow(dead_code)]
/// The maximum amount of headers parsed on the server.
const MAX_HEADERS: usize = 128;

/// The maximum length of the head section we'll try to parse.
/// See: https://nodejs.org/en/blog/vulnerability/november-2018-security-releases/#denial-of-service-with-large-http-headers-cve-2018-12121
const MAX_HEAD_LENGTH: usize = 8 * 1024;

mod body_encoder;
mod chunked_encoder;
mod request_body;

pub use chunked_encoder::ChunkedEncoder;
pub use request_body::RequestBody;

pub mod server;
pub use server::{Server, ServerOptions};

mod error;
pub use error::{Error, Result};
pub use futures_lite::io::Cursor;

mod conn;
pub use conn::Conn;

mod upgrade;
pub use upgrade::Upgrade;

/// like ready! but early-returns the Poll<Result<usize>> early in all situations other than Ready(Ok(0))
#[macro_export]
macro_rules! read_to_end {
    ($expr:expr) => {
        match $expr {
            Poll::Ready(Ok(0)) => (),
            other => return other,
        }
    };
}
