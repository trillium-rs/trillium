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

//! testing utilities for trillium applications.
//!
//! this crate is intended to be used as a development dependency.
//!
//! ```
//! use trillium::{conn_try, Conn};
//! use trillium_testing::prelude::*;
//! async fn handler(mut conn: Conn) -> Conn {
//!     let request_body = conn_try!(conn.request_body_string().await, conn);
//!     conn.with_body(format!("request body was: {}", request_body))
//!         .with_status(418)
//!         .with_response_header("request-id", "special-request")
//! }
//!
//! assert_response!(
//!     post("/").with_request_body("hello trillium!").on(&handler),
//!     Status::ImATeapot,
//!     "request body was: hello trillium!",
//!     "request-id" => "special-request",
//!     "content-length" => "33"
//! );
//! ```
//!
//! ## Features
//!
//! You must enable a runtime feature for **trillium testing**
//!
//! ### Tokio:
//! ```toml
//! [dev-dependencies]
//! # ...
//! trillium-testing = { version = "0.2", features = ["tokio"] }
//! ```
//!
//! ### Async-std:
//! ```toml
//! [dev-dependencies]
//! # ...
//! trillium-testing = { version = "0.2", features = ["async-std"] }
//! ```
//!
//! ### Smol:
//! ```toml
//! [dev-dependencies]
//! # ...
//! trillium-testing = { version = "0.2", features = ["smol"] }
//! ```

mod assertions;

mod test_transport;
use std::{future::Future, process::Termination};
pub use test_transport::TestTransport;

mod test_conn;
pub use test_conn::TestConn;

pub mod methods;
pub mod prelude {
    //! useful stuff for testing trillium apps
    pub use crate::{
        assert_body, assert_body_contains, assert_headers, assert_not_handled, assert_ok,
        assert_response, assert_status, block_on, connector, init, methods::*,
    };
    pub use trillium::{Conn, Method, Status};
}

pub use trillium::{Method, Status};
pub use url::Url;

/// runs the future to completion on the current thread
pub fn block_on<Fut: Future>(fut: Fut) -> Fut::Output {
    runtime().block_on(fut)
}

/// initialize a handler
pub fn init(handler: &mut impl trillium::Handler) {
    let mut info = "testing".into();
    block_on(async move { handler.init(&mut info).await })
}

// these exports are used by macros
pub use futures_lite::{self, AsyncRead, AsyncReadExt, AsyncWrite, Stream};

mod server_connector;
pub use server_connector::{connector, ServerConnector};
use trillium_server_common::Config;
pub use trillium_server_common::{
    ArcedConnector, Connector, Runtime, RuntimeTrait, Server, ServerHandle,
};

cfg_if::cfg_if! {
    if #[cfg(feature = "smol")] {
        /// runtime server config
        pub fn config() -> Config<impl Server, ()> {
            trillium_smol::config()
        }

        /// runtime client config
        pub fn client_config() -> impl Connector {
            trillium_smol::ClientConfig::default()
        }
        /// smol runtime
        pub fn runtime() -> impl RuntimeTrait {
            trillium_smol::SmolRuntime::default()
        }
        pub(crate) use trillium_smol::SmolRuntime as RuntimeType;
    } else if #[cfg(feature = "async-std")] {
        /// runtime server config
        pub fn config() -> Config<impl Server, ()> {
            trillium_async_std::config()
        }
        /// runtime client config
        pub fn client_config() -> impl Connector {
            trillium_async_std::ClientConfig::default()
        }
        /// async std runtime
        pub fn runtime() -> impl RuntimeTrait {
            trillium_async_std::AsyncStdRuntime::default()
        }
        pub(crate) use trillium_async_std::AsyncStdRuntime as RuntimeType;

    } else if #[cfg(feature = "tokio")] {
        /// runtime server config
        pub fn config() -> Config<impl Server, ()> {
            trillium_tokio::config()
        }

        /// tokio client config
        pub fn client_config() -> impl Connector {
            trillium_tokio::ClientConfig::default()
        }

        /// tokio runtime
        pub fn runtime() -> impl RuntimeTrait {
            trillium_tokio::TokioRuntime::default()
        }

        pub(crate) use trillium_tokio::TokioRuntime as RuntimeType;
   } else {
        /// runtime server config
        pub fn config() -> Config<impl Server, ()> {
            Config::<RuntimelessServer, ()>::new()
        }

        /// generic client config
        pub fn client_config() -> impl Connector {
            RuntimelessClientConfig::default()
        }

       /// generic runtime
       pub fn runtime() -> impl RuntimeTrait {
           RuntimelessRuntime::default()
       }

       pub(crate) use RuntimelessRuntime as RuntimeType;
   }
}

mod with_server;
pub use with_server::{with_server, with_transport};

mod runtimeless;
pub use runtimeless::{RuntimelessClientConfig, RuntimelessRuntime, RuntimelessServer};

/// a sponge Result
pub type TestResult = Result<(), Box<dyn std::error::Error>>;

/// a test harness for use with [`test_harness`]
#[track_caller]
pub fn harness<F, Fut, Output>(test: F) -> Output
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Output>,
    Output: Termination,
{
    block_on(test())
}
