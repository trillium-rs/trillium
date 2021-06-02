#![forbid(unsafe_code)]
#![warn(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
//    missing_docs,
    nonstandard_style,
    unused_qualifications
)]
pub use async_native_tls;
pub use native_tls;
pub use native_tls::Identity;

mod server;
pub use server::NativeTlsAcceptor;

mod client;
pub use client::{NativeTlsConfig, NativeTlsConnector, NativeTlsTransport};
