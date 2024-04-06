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

/*!
testing utilities for trillium applications.

this crate is intended to be used as a development dependency.

```
use trillium_testing::prelude::*;
use trillium::{Conn, conn_try};
async fn handler(mut conn: Conn) -> Conn {
    let request_body = conn_try!(conn.request_body_string().await, conn);
    conn.with_body(format!("request body was: {}", request_body))
        .with_status(418)
        .with_response_header("request-id", "special-request")
}

assert_response!(
    post("/").with_request_body("hello trillium!").on(&handler),
    Status::ImATeapot,
    "request body was: hello trillium!",
    "request-id" => "special-request",
    "content-length" => "33"
);

```

## Features

**You must enable a runtime feature for trillium testing**

### Tokio:
```toml
[dev-dependencies]
# ...
trillium-testing = { version = "0.2", features = ["tokio"] }
```

### Async-std:
```toml
[dev-dependencies]
# ...
trillium-testing = { version = "0.2", features = ["async-std"] }
```

### Smol:
```toml
[dev-dependencies]
# ...
trillium-testing = { version = "0.2", features = ["smol"] }
```


*/

mod assertions;

mod test_transport;
use std::future::{Future, IntoFuture};
use std::process::Termination;

pub use test_transport::TestTransport;

mod test_conn;
pub use test_conn::TestConn;

pub mod methods;
pub mod prelude {
    /*!
    useful stuff for testing trillium apps
    */
    pub use crate::{
        assert_body, assert_body_contains, assert_headers, assert_not_handled, assert_ok,
        assert_response, assert_status, block_on, connector, init, methods::*,
    };

    pub use trillium::{Conn, Method, Status};
}

pub use trillium::{Method, Status};

pub use url::Url;

/// initialize a handler
pub fn init(handler: &mut impl trillium::Handler) {
    let mut info = "testing".into();
    block_on(handler.init(&mut info))
}

// these exports are used by macros
pub use futures_lite;
pub use futures_lite::{AsyncRead, AsyncReadExt, AsyncWrite};

mod server_connector;
pub use server_connector::{connector, ServerConnector};

use trillium_server_common::Config;
pub use trillium_server_common::{ArcedConnector, Connector, Server, ServerHandle};

#[derive(Debug)]
/// A droppable future
///
/// This only exists because of the #[must_use] on futures. The task will run to completion whether
/// or not this future is awaited.
pub struct SpawnHandle<F>(F);
impl<F> IntoFuture for SpawnHandle<F>
where
    F: Future,
{
    type IntoFuture = F;
    type Output = F::Output;
    fn into_future(self) -> Self::IntoFuture {
        self.0
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "smol")] {
        /// runtime server config
        pub fn config() -> Config<impl Server, ()> {
            trillium_smol::config()
        }

        /// smol-based spawn variant that finishes whether or not the returned future is dropped
        pub fn spawn<Fut, Out>(future: Fut) -> SpawnHandle<impl Future<Output = Option<Out>>>
        where
            Fut: Future<Output = Out> + Send + 'static,
            Out: Send + 'static
        {
            let (tx, rx) = async_channel::bounded::<Out>(1);
            trillium_smol::async_global_executor::spawn(async move { let _ = tx.send(future.await).await; }).detach();
            SpawnHandle(async move {
                let rx = rx;
                rx.recv().await.ok()
            })
        }

        /// runtime client config
        pub fn client_config() -> impl Connector {
            ClientConfig::default()
        }
        pub use trillium_smol::async_global_executor::block_on;
        pub use trillium_smol::ClientConfig;

    } else if #[cfg(feature = "async-std")] {
        /// runtime server config
        pub fn config() -> Config<impl Server, ()> {
            trillium_async_std::config()
        }
        pub use trillium_async_std::async_std::task::block_on;
        pub use trillium_async_std::ClientConfig;

        /// async-std-based spawn variant that finishes whether or not the returned future is dropped
        pub fn spawn<Fut, Out>(future: Fut) -> SpawnHandle<impl Future<Output = Option<Out>>>
        where
            Fut: Future<Output = Out> + Send + 'static,
            Out: Send + 'static
        {
            let (tx, rx) = async_channel::bounded::<Out>(1);
            trillium_async_std::async_std::task::spawn(async move { let _ = tx.send(future.await).await; });
            SpawnHandle(async move {
                let rx = rx;
                rx.recv().await.ok()
            })
        }
        /// runtime client config
        pub fn client_config() -> impl Connector {
            ClientConfig::default()
        }
    } else if #[cfg(feature = "tokio")] {
        /// runtime server config
        pub fn config() -> Config<impl Server, ()> {
            trillium_tokio::config()
        }
        pub use trillium_tokio::ClientConfig;
        pub use trillium_tokio::block_on;
        /// tokio-based spawn variant that finishes whether or not the returned future is dropped
        pub fn spawn<Fut, Out>(future: Fut) -> SpawnHandle<impl Future<Output = Option<Out>>>
        where
            Fut: Future<Output = Out> + Send + 'static,
            Out: Send + 'static
        {
            let (tx, rx) = async_channel::bounded::<Out>(1);
            trillium_tokio::tokio::task::spawn(async move { let _ = tx.send(future.await).await; });
            SpawnHandle(async move {
                let rx = rx;
                rx.recv().await.ok()
            })
        }
        /// runtime client config
        pub fn client_config() -> impl Connector {
            ClientConfig::default()
        }
   } else {
        /// runtime server config
        pub fn config() -> Config<impl Server, ()> {
            Config::<RuntimelessServer, ()>::new()
        }

        pub use RuntimelessClientConfig as ClientConfig;

        /// generic client config
        pub fn client_config() -> impl Connector {
            RuntimelessClientConfig::default()
        }

        pub use futures_lite::future::block_on;

        /// fake runtimeless spawn that finishes whether or not the future is dropped
        pub fn spawn<Fut, Out>(future: Fut) -> SpawnHandle<impl Future<Output = Option<Out>>>
        where
            Fut: Future<Output = Out> + Send + 'static,
            Out: Send + 'static
        {
            let (tx, rx) = async_channel::bounded::<Out>(1);
            std::thread::spawn(move || { let _ = tx.send_blocking(block_on(future)); });
            SpawnHandle(async move {
                let rx = rx;
                rx.recv().await.ok()
            })
        }
    }
}

mod with_server;
pub use with_server::{with_server, with_transport};

mod runtimeless;
pub use runtimeless::{RuntimelessClientConfig, RuntimelessServer};

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
