#![doc = include_str!("../docs/root.md")]
#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

mod api_conn_ext;
mod api_handler;
mod before_send;
mod body;
mod cancel_on_disconnect;
mod error;
mod from_conn;
mod halt;
#[cfg(any(feature = "serde_json", feature = "sonic-rs"))]
mod json;
mod state;
mod try_from_conn;

pub use api_conn_ext::ApiConnExt;
pub use api_handler::{ApiHandler, api};
pub use before_send::BeforeSend;
pub use body::Body;
pub use cancel_on_disconnect::{CancelOnDisconnect, cancel_on_disconnect};
pub use error::Error;
pub use from_conn::FromConn;
pub use halt::Halt;
#[cfg(any(feature = "serde_json", feature = "sonic-rs"))]
pub use json::Json;

#[cfg(all(feature = "serde_json", feature = "sonic-rs"))]
compile_error!("cargo features \"serde_json\" and \"sonic-rs\" are mutually exclusive");

#[cfg(feature = "serde_json")]
pub use serde_json::{Value, json};
#[cfg(feature = "sonic-rs")]
pub use sonic_rs::{Value, json};
pub use state::State;
pub use try_from_conn::TryFromConn;

/// trait alias for a result with this crate's [`Error`]
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(doc)]
#[doc = include_str!("../docs/extractors.md")]
pub mod extractors {
    #[doc = include_str!("../docs/extractors/custom.md")]
    pub mod custom {}
}

#[cfg(doc)]
#[doc = include_str!("../docs/return_types.md")]
pub mod return_types {}

#[cfg(doc)]
#[doc = include_str!("../docs/error_handling.md")]
pub mod error_handling {}

#[cfg(doc)]
#[doc = include_str!("../docs/recipes.md")]
pub mod recipes {}
