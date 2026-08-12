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

//! # this crate provides the tera templating language for trillium
//!
//! See [the tera site](https://keats.github.io/tera/) for more information on
//! the tera template language.
//!
//! This crate tracks tera 2, and enables tera's `fast` feature by default. The
//! `preserve_order` and `unicode` tera features are also available as passthroughs.
//!
//! ```
//! # fn main() -> tera::TeraResult<()> {
//! use trillium::Conn;
//! use trillium_tera::{Tera, TeraConnExt, TeraHandler};
//! use trillium_testing::TestServer;
//!
//! let mut tera = Tera::default();
//! tera.add_raw_template("hello.html", "hello {{name}} from {{render_engine}}")?;
//!
//! let handler = (
//!     TeraHandler::new(tera),
//!     |conn: Conn| async move { conn.assign("render_engine", "tera") },
//!     |conn: Conn| async move { conn.assign("name", "trillium").render("hello.html") },
//! );
//!
//! # trillium_testing::block_on(async {
//! let app = TestServer::new(handler).await;
//! app.get("/")
//!     .await
//!     .assert_ok()
//!     .assert_body("hello trillium from tera")
//!     .assert_header("content-type", "text/html");
//! # });
//! # Ok(()) }
//! ```

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

mod tera_handler;
pub use tera_handler::TeraHandler;

mod tera_conn_ext;
pub use tera::{Context, Error, Filter, Function, Tera, TeraResult, Test};
pub use tera_conn_ext::TeraConnExt;
