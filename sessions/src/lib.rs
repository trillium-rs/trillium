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
```
use trillium::Conn;
use trillium_cookies::{CookiesHandler, cookie::Cookie};
use trillium_sessions::{MemoryStore, SessionConnExt, SessionHandler};

let handler = (
    CookiesHandler,
    SessionHandler::new(MemoryStore::new(), b"you should use an env var instead of a string literal"),
    |conn: Conn| async move {
        let count: usize = conn.session().get("count").unwrap_or_default();
        conn.with_session("count", count + 1)
            .ok(format!("count: {}", count))
    },
);

use trillium_testing::{TestHandler, TestConn, assert_ok};
let test_handler = TestHandler::new(handler);
let mut conn = test_handler.get("/");
assert_ok!(&mut conn, "count: 0");

let set_cookie_header = conn.headers_mut().get("set-cookie").unwrap().as_str();
let cookie = Cookie::parse_encoded(set_cookie_header).unwrap();

let make_request = || TestConn::get("/")
    .with_header(("cookie", &*format!("{}={}", cookie.name(), cookie.value())))
    .run(&test_handler);

assert_ok!(make_request(), "count: 1");
assert_ok!(make_request(), "count: 2");
assert_ok!(make_request(), "count: 3");
assert_ok!(make_request(), "count: 4");
```
*/

mod session_conn_ext;
pub use session_conn_ext::SessionConnExt;

mod session_handler;
pub use session_handler::SessionHandler;

pub use async_session::{CookieStore, MemoryStore, Session};
