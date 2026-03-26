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

//! provides support for using the askama compile-time template library
//! with trillium.  see
//! [https://github.com/djc/askama](https://github.com/djc/askama) for
//! more information about using askama.
//!
//! ```
//! use trillium::Conn;
//! use trillium_askama::{AskamaConnExt, Template};
//! use trillium_testing::TestServer;
//!
//! #[derive(Template)]
//! #[template(path = "examples/hello.html")]
//! struct HelloTemplate<'a> {
//!     name: &'a str,
//! }
//!
//! async fn handler(conn: Conn) -> Conn {
//!     conn.render(HelloTemplate { name: "trillium" })
//! }
//!
//! # trillium_testing::block_on(async {
//! let app = TestServer::new(handler).await;
//! app.get("/")
//!     .await
//!     .assert_ok()
//!     .assert_body("Hello, trillium!\n");
//! # });
//! ```

pub use askama::{self, Template};
use trillium::Status;

/// extends trillium conns with the ability to render askama templates
pub trait AskamaConnExt {
    /// renders an askama template, halting the conn and setting a 200
    /// status code. also sets the mime type based on the template
    /// extension
    fn render(self, template: impl Template) -> Self;
}

impl AskamaConnExt for trillium::Conn {
    fn render(self, template: impl Template) -> Self {
        match template.render() {
            Ok(text) => self.ok(text),
            Err(error) => {
                log::error!("Askama render error: {error}");
                self.with_status(Status::InternalServerError)
                    .with_state(error)
            }
        }
    }
}

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}
