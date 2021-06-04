#![forbid(unsafe_code)]
#![warn(
    missing_copy_implementations,
    missing_crate_level_docs,
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

async fn handler(mut conn: Conn) -> Conn {
    let request_body = conn.request_body().await.read_string().await.unwrap();
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
        assert_response, assert_status, methods::*, Method, StatusCode,
    };

    pub use trillium::Conn;
}

// these exports are used by macros
pub use futures_lite::{future::block_on, AsyncRead, AsyncReadExt, AsyncWrite};
pub use trillium_http::http_types::{Method, StatusCode, Url};
