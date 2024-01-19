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
provides support for using the askama compile-time template library
with trillium.  see
[https://github.com/djc/askama](https://github.com/djc/askama) for
more information about using askama.

```
use trillium::Conn;
use trillium_askama::{AskamaConnExt, Template};

#[derive(Template)]
#[template(path = "examples/hello.html")]
struct HelloTemplate<'a> {
    name: &'a str,
}

async fn handler(conn: Conn) -> Conn {
    conn.render(HelloTemplate { name: "trillium" })
}

use trillium_testing::prelude::*;
assert_ok!(
    get("/").on(&handler),
    "Hello, trillium!",
    "content-type" => "text/html"
);
```
*/

pub use askama;
pub use askama::Template;
use trillium::KnownHeaderName::ContentType;

/// extends trillium conns with the ability to render askama templates
pub trait AskamaConnExt {
    /// renders an askama template, halting the conn and setting a 200
    /// status code. also sets the mime type based on the template
    /// extension
    fn render(self, template: impl Template) -> Self;
}

impl AskamaConnExt for trillium::Conn {
    fn render(mut self, template: impl Template) -> Self {
        use askama::DynTemplate;
        let text = template.render().unwrap();
        if let Some(extension) = template.extension() {
            if let Some(mime) = mime_guess::from_ext(extension).first_raw() {
                self.response_headers_mut().try_insert(ContentType, mime);
            }
        }

        self.ok(text)
    }
}
