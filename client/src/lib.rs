#![forbid(unsafe_code)]
#![warn(
    missing_copy_implementations,
//    missing_crate_level_docs,
    missing_debug_implementations,
//    missing_docs,
    nonstandard_style,
    unused_qualifications
)]
mod conn;
pub use conn::Conn;

mod pool;
pub use pool::Pool;

mod native_tls_transport;
pub use native_tls_transport::{NativeTls, NativeTlsConfig};

mod rustls_transport;
pub use rustls_transport::{Rustls, RustlsConfig};

mod client;
pub use client::Client;

mod transport;
pub use transport::{ClientTransport, TcpConfig};

pub use async_net::TcpStream;
pub use trillium_http::{Error, Result};

mod util;
