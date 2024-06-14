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

/*!  This crate provides rustls trait implementations for trillium client ([`RustlsConnector`]) and
server ([`RustlsAcceptor`]).

# Cargo Features

This crate's default features should be appropriate for most users. To pare down on dependencies or
customize trillium-rustls' usage of rustls, opt out of default features and reenable the appropriate
features for your use case.

## `server` and `client` features

This crate offers a `server` feature and a `client` feature. Opting out of default features allows
you to avoid building any dependencies for the unused other component. By default, both `server` and
`client` features are enabled.

## Cryptographic backend selection

Rustls supports pluggable cryptographic backends as well as a process-default cryptographic
cryptographic backend. There are two built-in feature-enabled cryptographic backends and other
community provided cryptographic backends.

⚠️ There are three cryptographic backend cargo features, and they behave differently than the rustls
features. Please read the following section.⚠️

`trillium-rustls` tries to avoid runtime panics where possible, so compiling this crate without a
valid cryptographic backend will result in a compile time error. To opt into rustls's default
process-default behavior, enable `custom-crypto-provider` as described below. Enabling multiple
crypto providers will select exactly one of them at compile time in the following order:

### `aws-lc-rs`

This is the default cryptographic backend in concordance with rustls' default. This backend will be
selected if the feature is enabled. If either of the other two cryptographic backends are selected,
trillium-rustls will log an error but use `aws-lc-rs`.

### `ring`

If this feature is enabled, this backend will be selected even if `custom-crypto-provider` is also
enabled.

### `custom-crypto-provider`

In order to use a crypto provider other than the above two options, enable the
`custom-crypto-provider` feature and either configure a
[`trillium_rustls::rustls::ClientConfig`][rustls::ClientConfig] or
[`trillium_rustls::rustls::ServerConfig`][rustls::ServerConfig] yourself to convert the equivalent
`trillium-rustls` type, or install a custom process-default crypto provider with
[`trillium_rustls::rustls::crypto::CryptoProvider::install_default`][rustls::crypto::CryptoProvider::install_default]
prior to executing trillium-rustls code.

## Client verifier

This crate offers a `platform-verifier` feature for client usage that builds a ClientConfig with the
selected cryptographic backend and uses
[`rustls-platform-verifier`](https://docs.rs/rustls-platform-verifier/). This feature is enabled by
default. If you disable the feature, [`webpki_roots`] will be used.
*/

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "client")]
pub use client::{RustlsClientTransport, RustlsConfig};

#[cfg(feature = "server")]
mod server;
pub use futures_rustls::{self, rustls};
#[cfg(feature = "server")]
pub use server::{RustlsAcceptor, RustlsServerTransport};

#[cfg(any(feature = "client", feature = "server"))]
mod crypto_provider;
pub(crate) use crypto_provider::crypto_provider;
