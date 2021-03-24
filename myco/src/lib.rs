//! Welcome to myco!
//!
//! This crate is the primary and minimum dependency for building a
//! myco app or library. It contains a handful of core types and
//! reexports a few others that you will necessarily need, but
//! otherwise tries to stay small and focused. This crate will
//! hopefully be the most stable within the myco ecosystem.
//!
//!
mod handler;
pub use handler::{Handler, Sequence};

mod conn;
pub use conn::Conn;

mod state;
pub use state::State;

pub use async_trait::async_trait;
pub use myco_http::http_types;

mod transport;
pub use transport::{BoxedTransport, Transport};

pub type Upgrade = myco_http::Upgrade<BoxedTransport>;

mod macros;
