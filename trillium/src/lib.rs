#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
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
pub use handler::Handler;

mod conn;
pub use conn::Conn;

mod state;
pub use state::State;

pub use async_trait::async_trait;
pub use trillium_http::http_types;

mod transport;
pub use transport::Transport;

mod boxed_transport;
pub use boxed_transport::BoxedTransport;

/// An [`Upgrade`](trillium_http::Upgrade) for [`BoxedTransport`]s
///
/// This exists to erase the generic transport for convenience. See
/// [`Upgrade`](trillium_http::Upgrade) for additional documentation
pub type Upgrade = trillium_http::Upgrade<BoxedTransport>;

mod macros;

mod sequence;
pub use sequence::Sequence;
