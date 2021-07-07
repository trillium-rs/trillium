#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

/*!

# this crate provides the tera templating language for trillium

See [the tera site](https://tera.netlify.app/) for more information on
the tera template language.

```
# fn main() -> tera::Result<()> {
use trillium::Conn;
use trillium_tera::{TeraHandler, Tera, TeraConnExt};

let mut tera = Tera::default();
tera.add_raw_template("hello.html", "hello {{name}} from {{render_engine}}")?;

let handler = (
    TeraHandler::new(tera),
    |conn: Conn| async move { conn.assign("render_engine", "tera") },
    |conn: Conn| async move {
        conn.assign("name", "trillium").render("hello.html")
    }
);

use trillium_testing::prelude::*;
assert_ok!(
    get("/").on(&handler),
    "hello trillium from tera",
    "content-type" => "text/html"
);
# Ok(()) }
```
*/

mod tera_handler;
pub use tera_handler::TeraHandler;

mod tera_conn_ext;
pub use tera_conn_ext::TeraConnExt;

pub use tera::{Context, Tera};
