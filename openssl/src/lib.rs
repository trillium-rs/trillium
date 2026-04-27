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

//! This crate provides openssl trait implementations for trillium client ([`OpenSslConfig`]) and
//! server ([`OpenSslAcceptor`]).

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

pub use async_openssl;
pub use openssl;

mod alpn;

mod server;
pub use server::{OpenSslAcceptor, OpenSslServerTransport};

mod client;
pub use client::{OpenSslClientConfig, OpenSslClientTransport, OpenSslConfig};
