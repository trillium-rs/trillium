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

//! Handlebars templating handler for trillium based on [the handlebars
//! crate](https://docs.rs/crate/handlebars).
//! ```
//! # if cfg!(unix) {
//! # use std::path::PathBuf;
//! use trillium_handlebars::{HandlebarsConnExt, HandlebarsHandler};
//! let handler = (
//!     HandlebarsHandler::new("**/*.hbs"),
//!     |mut conn: trillium::Conn| async move {
//!         conn.assign("name", "handlebars")
//!             .render("examples/templates/hello.hbs")
//!     },
//! );
//!
//! use trillium_testing::prelude::*;
//! assert_ok!(get("/").on(&handler), "hello handlebars!");
//! # }
//! ```

pub use handlebars::{self, Handlebars};

mod assigns;
pub use assigns::Assigns;

mod handlebars_handler;
pub use handlebars_handler::HandlebarsHandler;

mod handlebars_conn_ext;
pub use handlebars_conn_ext::HandlebarsConnExt;
