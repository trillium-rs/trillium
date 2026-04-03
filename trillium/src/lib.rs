#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    nonstandard_style,
    unused_qualifications
)]
#![warn(missing_docs, clippy::pedantic, clippy::nursery, clippy::cargo)]
#![allow(
    clippy::must_use_candidate,
    clippy::module_name_repetitions,
    clippy::multiple_crate_versions
)]

//! # Welcome to the `trillium` crate!
//!
//! This crate is the primary dependency for building a trillium app or
//! library. It contains a handful of core types and reexports a few
//! others that you will necessarily need, but otherwise tries to stay
//! small and focused. This crate will hopefully be the most stable within
//! the trillium ecosystem. That said, trillium is still pre 1.0 and
//! should be expected to evolve over time.
//!
//! To get started with this crate, first take a look at [the
//! guide](https://trillium.rs), then browse the docs for
//! [`trillium::Conn`](crate::Conn).
//!
//! At a minimum to build a trillium app, you'll also need a trillium
//! [runtime adapter](https://trillium.rs/overview/runtimes.html).

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

mod handler;
pub use handler::Handler;

/// Server header.
///
/// The contents of this constant are necessarily unconstrained by semver.
pub const SERVER: &str = concat!("trillium/", env!("CARGO_PKG_VERSION"));

mod conn;
pub use conn::Conn;

mod state;
pub use state::{State, state};
pub use trillium_http::{
    Body, BodySource, Error, HeaderName, HeaderValue, HeaderValues, Headers, HttpConfig,
    KnownHeaderName, Method, ServerConfig, Status, Swansong, TypeSet, Version,
};

mod transport;
pub use transport::Transport;

mod upgrade;
pub use upgrade::Upgrade;

mod macros;

pub use log;

mod info;
pub use info::Info;

mod boxed_handler;
pub use boxed_handler::BoxedHandler;

mod init;
pub use init::{Init, init};

/// Types for interacting with [`Headers`]
pub mod headers {
    pub use trillium_http::headers::{Entry, IntoIter, Iter};
}

/// Types for interacting with [`TypeSet`]
pub mod type_set {
    pub use trillium_http::type_set::entry::Entry;
}

mod request_body;
pub use request_body::RequestBody;
