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
This crate provides native tls trait implementations for trillium
client ([`NativeTlsConnector`]) and server ([`NativeTlsAcceptor`]).
*/

pub use async_native_tls;
pub use native_tls::{self, Identity};

mod server;
pub use server::{NativeTlsAcceptor, NativeTlsServerTransport};

mod client;
pub use client::{NativeTlsClientTransport, NativeTlsConfig};
