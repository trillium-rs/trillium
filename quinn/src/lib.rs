//! Quinn-backed QUIC adapter for Trillium HTTP/3.
//!
//! This crate provides [`QuicConfig`], which enables HTTP/3 over QUIC using the
//! [quinn](https://docs.rs/quinn) library alongside any Trillium server adapter.
//!
//! # Crypto provider
//!
//! TLS is required for HTTP/3. Select a crypto provider feature:
//! - `aws-lc-rs` (default)
//! - `ring` — use the ring crypto library instead
//! - `custom-crypto-provider` — bring your own provider via
//!   `rustls::crypto::CryptoProvider::install_default`

mod config;
mod connection;
mod crypto_provider;
mod runtime;

pub use config::QuicConfig;
