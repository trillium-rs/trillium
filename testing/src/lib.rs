#![forbid(unsafe_code)]
#![warn(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

mod assertions;

mod test_io;
pub use test_io::{CloseableCursor, TestTransport};

mod test_conn;
pub use test_conn::TestConn;

mod serve_once;
pub use serve_once::serve_once;

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
