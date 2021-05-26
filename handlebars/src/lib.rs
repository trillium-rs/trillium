#![forbid(unsafe_code)]
#![warn(
    missing_copy_implementations,
    missing_crate_level_docs,
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
//! use trillium_handlebars::{HandlebarsHandler, HandlebarsConnExt};
//! let handler = (
//!     HandlebarsHandler::new("**/*.hbs"),
//!     |mut conn: trillium::Conn| async move {
//!         conn.assign("name", "handlebars")
//!             .render("examples/templates/hello.hbs")
//!     }
//! );
//!
//! use trillium_testing::{TestHandler, assert_ok};
//! let test_handler = TestHandler::new(handler);
//! assert_ok!(test_handler.get("/"), "hello handlebars!");
//! # }
//! ```

pub use handlebars;
pub use handlebars::Handlebars;

mod assigns;
pub use assigns::Assigns;

mod handlebars_handler;
pub use handlebars_handler::HandlebarsHandler;

mod handlebars_conn_ext;
pub use handlebars_conn_ext::HandlebarsConnExt;
