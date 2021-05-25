#![forbid(unsafe_code)]
#![warn(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

pub use handlebars;

mod assigns;
pub use assigns::Assigns;

mod handlebars_handler;
pub use handlebars_handler::Handlebars;

mod handlebars_conn_ext;
pub use handlebars_conn_ext::HandlebarsConnExt;
