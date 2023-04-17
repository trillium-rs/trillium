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
# Utilities and traits for building trillium runtime adapters

Trillium applications should never need to depend directly on this
library. Server adapters should reexport any types from this crate
that an application author would need to use.

The parts of this crate that are not application facing should be
expected to change more frequently than the parts that are application
facing.

If you are depending on this crate for private code that cannot be
discovered through docs.rs' reverse dependencies, please open an
issue.
*/
pub use async_trait::async_trait;
pub use futures_lite::{AsyncRead, AsyncWrite};
pub use trillium_http::{transport::Transport, Stopper};
pub use url::Url;

mod clone_counter;
pub use clone_counter::CloneCounter;

mod config;
pub use config::Config;

mod config_ext;
pub use config_ext::ConfigExt;

mod server;
pub use server::Server;

mod binding;
pub use binding::Binding;

mod client;
pub use client::{Connector, ObjectSafeConnector};

mod acceptor;
pub use acceptor::Acceptor;
