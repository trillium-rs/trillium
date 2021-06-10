#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

/*!
# Welcome to the `trillium` crate!

This crate is the primary dependency for building a trillium app or
library. It contains a handful of core types and reexports a few
others that you will necessarily need, but otherwise tries to stay
small and focused. This crate will hopefully be the most stable within
the trillium ecosystem. That said, trillium is still pre 1.0 and
should be expected to evolve over time.

To get started with this crate, first take a look at [the
guide](https://trillium.rs), then browse the docs for
[`trillium::Conn`](crate::Conn).

At a minimum to build a trillium app, you'll also need a trillium
[runtime adapter](https://trillium.rs/overview/runtimes.html).

*/
mod handler;
pub use handler::Handler;

mod conn;
pub use conn::Conn;

mod state;
pub use state::State;

pub use async_trait::async_trait;
pub use trillium_http::http_types;

/**
# A HTTP protocol upgrade

This exists to erase the generic transport for convenience using a
[BoxedTransport](trillium_http::transport::BoxedTransport). See
[`Upgrade`](trillium_http::Upgrade) for additional documentation
*/
pub type Upgrade = trillium_http::Upgrade<trillium_http::transport::BoxedTransport>;

mod macros;

pub use log;

mod info;
pub use info::Info;

mod init;
pub use init::Init;
