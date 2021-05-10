#![warn(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

pub use trillium_http::Stopper;
pub use trillium_tls_common::Acceptor;

mod clone_counter;
pub use clone_counter::CloneCounter;

mod config;
pub use config::Config;

mod config_ext;
pub use config_ext::ConfigExt;

mod server;
pub use server::Server;
