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
# Welcome to the trillium router crate!

This router is built on top of
[routefinder](https://github.com/jbr/routefinder), and the details of
route resolution and definition are documented on that repository.

```
use trillium::{conn_unwrap, Conn};
use trillium_router::{Router, RouterConnExt};

let router = Router::new()
    .get("/", |conn: Conn| async move { conn.ok("you have reached the index") })
    .get("/pages/:page_name", |conn: Conn| async move {
        let page_name = conn_unwrap!(conn, conn.param("page_name"));
        let content = format!("you have reached the page named {}", page_name);
        conn.ok(content)
    });

use trillium_testing::{TestHandler, assert_ok};
let test_handler = TestHandler::new(router);
assert_ok!(test_handler.get("/"), "you have reached the index");
assert_ok!(test_handler.get("/pages/trillium"), "you have reached the page named trillium");
assert!(test_handler.get("/unknown/route").status().is_none());
```

Although this is currently the only trillium router, it is an
important aspect of trillium's architecture that the router uses only
public apis and is interoperable with other router implementations. If
you have different ideas of how a router might work, please publish a
crate! It should be possible to nest different types of routers (and
different versions of router crates) within each other as long as they
all depend on the same version of the `trillium` crate.

*/

mod router;
pub use router::Router;

mod router_ref;
pub use router_ref::RouterRef;

mod router_conn_ext;
pub use router_conn_ext::RouterConnExt;

/**
The routes macro represents an experimental macro for defining
routers.

**stability note:** this may be removed entirely if it is not widely
used. please open an issue if you like it, or if you have ideas to
improve it.

```
use trillium::{conn_unwrap, Conn};
use trillium_router::{routes, RouterConnExt};

let router = routes!(
    get "/" |conn: Conn| async move { conn.ok("you have reached the index") },
    get "/pages/:page_name" |conn: Conn| async move {
        let page_name = conn_unwrap!(conn, conn.param("page_name"));
        let content = format!("you have reached the page named {}", page_name);
        conn.ok(content)
    }
);

use trillium_testing::{TestHandler, assert_ok};
let test_handler = TestHandler::new(router);
assert_ok!(test_handler.get("/"), "you have reached the index");
assert_ok!(test_handler.get("/pages/trillium"), "you have reached the page named trillium");
assert!(test_handler.get("/unknown/route").status().is_none());
```
*/
#[macro_export]
macro_rules! routes {
    ($($method:ident $path:literal $(-> )?$handler:expr),+ $(,)?) => {
	$crate::Router::new()$(
            .$method($path, $handler)
        )+;
    };
}
