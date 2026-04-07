#![cfg_attr(docsrs, feature(doc_cfg))]
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

//! Testing utilities for trillium applications.
//!
//! This crate is intended to be used as a development dependency.
//!
//! ```
//! # trillium_testing::block_on(async {
//! use trillium::{Conn, Status, conn_try};
//! use trillium_testing::TestServer;
//! async fn handler(mut conn: Conn) -> Conn {
//!     let request_body = conn_try!(conn.request_body_string().await, conn);
//!     conn.with_body(format!("request body was: {}", request_body))
//!         .with_status(418)
//!         .with_response_header("request-id", "special-request")
//! }
//!
//! let app = TestServer::new(handler).await;
//! app.post("/")
//!     .with_body("hello trillium!")
//!     .await
//!     .assert_status(Status::ImATeapot)
//!     .assert_body("request body was: hello trillium!")
//!     .assert_headers([("request-id", "special-request"), ("content-length", "33")]);
//! # });
//! ```
//!
//! ## Features
//!
//! To use the same runtime as your application, enable a runtime feature for **trillium testing**.
//! Without a runtime feature enabled, trillium testing will approximate a runtime through spawning
//! a thread per task and blocking on a future. This is fine for simple testing, but you probably
//! want to enable a server feature.
//!
//! ### Tokio:
//! ```toml
//! [dev-dependencies]
//! # ...
//! trillium-testing = { version = "...", features = ["tokio"] }
//! ```
//!
//! ### Async-std:
//! ```toml
//! [dev-dependencies]
//! # ...
//! trillium-testing = { version = "...", features = ["async-std"] }
//! ```
//!
//! ### Smol:
//! ```toml
//! [dev-dependencies]
//! # ...
//! trillium-testing = { version = "...", features = ["smol"] }
//! ```

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

mod assertions;

mod test_transport;
use std::{future::Future, process::Termination, sync::Arc};
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

use trillium::{Handler, Info};
pub use trillium::{Method, Status};
use trillium_http::HttpContext;
pub use url::Url;

/// runs the future to completion on the current thread
pub fn block_on<Fut: Future>(fut: Fut) -> Fut::Output {
    runtime().block_on(fut)
}

/// initialize a handler
pub async fn init(handler: &mut impl Handler) -> Arc<HttpContext> {
    let mut info = Info::from(HttpContext::default());
    info.insert_shared_state(runtime());
    info.insert_shared_state(runtime().into());
    handler.init(&mut info).await;
    Arc::new(info.into())
}

// these exports are used by macros
pub use futures_lite::{self, AsyncRead, AsyncReadExt, AsyncWrite, Stream};

mod server_connector;
pub use server_connector::{ServerConnector, connector};
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
   }
}

mod with_server;
pub use with_server::{with_server, with_transport};

mod test_server;
pub use test_server::{ConnTest, TestServer};

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
    let _ = env_logger::builder().is_test(true).try_init();
    block_on(test())
}

/// a harness that includes the runtime
#[track_caller]
pub fn with_runtime<F, Fut, Output>(test: F) -> Output
where
    F: FnOnce(Runtime) -> Fut,
    Fut: Future<Output = Output>,
    Output: Termination,
{
    let runtime = runtime();
    runtime.clone().block_on(test(runtime.into()))
}

pub use test_harness::test;

mod http_test;
#[doc(hidden)]
pub use http_test::HttpTest;

#[cfg(all(feature = "serde_json", feature = "sonic-rs"))]
compile_error!("cargo features \"serde_json\" and \"sonic-rs\" are mutually exclusive");

#[cfg(feature = "serde_json")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde_json")))]
pub use serde_json::{Value, from_str as from_json_str, json, to_string as to_json_string};
#[cfg(feature = "sonic-rs")]
#[cfg_attr(docsrs, doc(cfg(feature = "sonic-rs")))]
pub use sonic_rs::{Value, from_str as from_json_str, json, to_string as to_json_string};
