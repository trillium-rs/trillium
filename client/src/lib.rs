#![forbid(unsafe_code)]
#![warn(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

mod conn;
pub use conn::Conn;

mod pool;
pub(crate) use pool::Pool;

mod client;
pub use client::Client;

pub use trillium_http::{http_types, Error, Result};

mod util;

pub use trillium_tls_common::Connector;
