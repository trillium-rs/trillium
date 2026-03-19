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

//! # the trillium cookie handler
//!
//! ## example
//! ```
//! use trillium::Conn;
//! use trillium_cookies::{CookiesConnExt, CookiesHandler, cookie::Cookie};
//! use trillium_testing::{TestHandler, harness};
//!
//! # trillium_testing::block_on(async {
//! async fn handler_that_uses_cookies(conn: Conn) -> Conn {
//!     let content = if let Some(cookie_value) = conn.cookies().get("some_cookie") {
//!         format!("current cookie value: {}", cookie_value.value())
//!     } else {
//!         String::from("no cookie value set")
//!     };
//!
//!     conn.with_cookie(("some_cookie", "some-cookie-value"))
//!         .ok(content)
//! }
//!
//! let handler = (CookiesHandler::new(), handler_that_uses_cookies);
//! let app = TestHandler::new(handler).await;
//!
//! app.get("/")
//!     .await
//!     .assert_ok()
//!     .assert_body("no cookie value set")
//!     .assert_header("set-cookie", "some_cookie=some-cookie-value");
//!
//! app.get("/")
//!     .with_request_header("cookie", "some_cookie=trillium")
//!     .await
//!     .assert_ok()
//!     .assert_body("current cookie value: trillium")
//!     .assert_header("set-cookie", "some_cookie=some-cookie-value");
//! # });
//! ```

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

mod cookies_handler;
pub use cookies_handler::{CookiesHandler, cookies};

mod cookies_conn_ext;
pub use cookie;
pub use cookies_conn_ext::CookiesConnExt;
