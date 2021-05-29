#![warn(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

/*!
# Utilities for building trillium server adapters

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
