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
    let request_body = conn_try!(conn, conn.request_body_string().await);
    conn.with_body(format!("request body was: {}", request_body))
        .with_status(418)
        .with_header(("request-id", "special-request"))
}

assert_response!(
    post("/").with_request_body("hello trillium!").on(&handler),
    StatusCode::ImATeapot,
    "request body was: hello trillium!",
    "request-id" => "special-request",
    "content-length" => "33"
);

```

## Features

The default runtime for trillium_testing is smol. To test a
trillium_tokio or trillium_async_std application:

### Tokio:

```toml
[dev-dependencies.trillium-testing]
version = "0.1"
default-features = false
features = ["tokio"]

# or:

[dev-dependencies]
# ...
trillium_testing = { version = "0.1", default-features = false, features = ["tokio"] }
```

### Async-std:
```toml
[dev-dependencies.trillium-testing]
version = "0.1"
default-features = false
features = ["async-std"]

# or:

[dev-dependencies]
# ...
trillium_testing = { version = "0.1", default-features = false, features = ["async-std"] }
```


*/

mod assertions;

mod test_transport;
pub use test_transport::TestTransport;

mod test_conn;
pub use test_conn::TestConn;

mod with_server;
pub use with_server::with_server;

pub mod methods;
pub mod prelude {
    /*!
    useful stuff for testing trillium apps
    */
    pub use crate::{
        assert_body, assert_body_contains, assert_headers, assert_not_handled, assert_ok,
        assert_response, assert_status, init, methods::*, Method, StatusCode,
    };

    pub use trillium::Conn;
}

/// initialize a handler
pub fn init(handler: &mut impl trillium::Handler) {
    let mut info = "testing".into();
    block_on(handler.init(&mut info))
}

// these exports are used by macros
pub use futures_lite::{AsyncRead, AsyncReadExt, AsyncWrite};
pub use trillium_http::http_types::{Method, StatusCode, Url};

cfg_if::cfg_if! {
    if #[cfg(feature = "tokio")] {
        pub use trillium_tokio::block_on;
    } else if #[cfg(feature = "async-std")] {
        pub use trillium_async_std::async_std::task::block_on;
    } else if #[cfg(feature = "smol")] {
        pub use trillium_smol::async_global_executor::block_on;
    } else {
        compile_error!("must enable smol, async-std, or tokio feature");
    }
}
