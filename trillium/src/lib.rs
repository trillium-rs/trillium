#![forbid(unsafe_code)]
#![deny(
    missing_debug_implementations,
    nonstandard_style,
    missing_copy_implementations,
    unused_qualifications
)]
//! Welcome to trillium!
//!
//! This crate is the primary and minimum dependency for building a
//! trillium app or library. It contains a handful of core types and
//! reexports a few others that you will necessarily need, but
//! otherwise tries to stay small and focused. This crate will
//! hopefully be the most stable within the trillium ecosystem.
//!
//!
mod handler;
pub use handler::{Handler, Sequence};

mod conn;
pub use conn::Conn;

mod state;
pub use state::State;

pub use async_trait::async_trait;
pub use trillium_http::http_types;

mod transport;
pub use transport::{BoxedTransport, Transport};

pub type Upgrade = trillium_http::Upgrade<BoxedTransport>;

mod macros;
