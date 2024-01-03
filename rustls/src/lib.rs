#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

/*!
This crate provides rustls trait implementations for trillium
client ([`RustlsConnector`]) and server ([`RustlsAcceptor`]).
*/

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "client")]
pub use client::RustlsConfig;

#[cfg(feature = "server")]
mod server;
#[cfg(feature = "server")]
pub use server::RustlsAcceptor;

pub use rustls;

mod transport;
pub use transport::RustlsTransport;
