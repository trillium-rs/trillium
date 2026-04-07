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
//! small and focused.
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

mod conn;
pub use conn::Conn;

mod state;
pub use state::{State, state};
pub use trillium_http::{
    Body, BodySource, Error, HeaderName, HeaderValue, HeaderValues, Headers, HttpConfig,
    HttpContext, KnownHeaderName, Method, Status, Swansong, TypeSet, Version,
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
    use crate::{CRATE_VERSION, HeaderValue};
    use std::sync::LazyLock;
    use trillium_http::CRATE_VERSION as HTTP_CRATE_VERSION;
    pub use trillium_http::headers::{Entry, IntoIter, Iter};

    /// Returns the default server header value for trillium, including both the
    /// `trillium` and `trillium-http` crate versions.
    ///
    /// The contents are necessarily unconstrained by semver.
    pub fn server_header() -> HeaderValue {
        static SERVER_HEADER: LazyLock<HeaderValue> = LazyLock::new(|| {
            let s: &'static str =
                format!("trillium/{CRATE_VERSION} trillium-http/{HTTP_CRATE_VERSION}").leak();
            s.into()
        });

        SERVER_HEADER.clone()
    }
}

/// Types for interacting with [`TypeSet`]
pub mod type_set {
    pub use trillium_http::type_set::entry::Entry;
}

mod request_body;
pub use request_body::RequestBody;

/// The version of this crate
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");
