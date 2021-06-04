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
# Trillium sessions

Trillium sessions is built on top of
[`async-session`](https://github.com/http-rs/async-session).

Sessions allows trillium to securely attach data to a browser session
allowing for retrieval and modification of this data within trillium
on subsequent visits. Session data is generally only retained for the
duration of a browser session.

Trillium's session implementation provides guest sessions by default,
meaning that all web requests to a session-enabled trillium host will
have a cookie attached, whether or not there is anything stored in
that client's session yet.

## Stores

Although this crate provides two bundled session stores, it is highly
recommended that trillium applications use an
external-datastore-backed session storage. For a list of currently
available session stores, see [the documentation for
async-session](https://github.com/http-rs/async-session).

## Security

Although each session store may have different security implications,
the general approach of trillium's session system is as follows: On
each request, trillium checks the cookie configurable as `cookie_name`
on the handler.

### If no cookie is found:

A cryptographically random cookie value is generated. A cookie is set
on the outbound response and signed with an HKDF key derived from the
`secret` provided on creation of the SessionHandler.  The configurable
session store uses a SHA256 digest of the cookie value and stores the
session along with a potential expiry.

### If a cookie is found:

The hkdf derived signing key is used to verify the cookie value's
signature. If it verifies, it is then passed to the session store to
retrieve a Session. For most session stores, this will involve taking
a SHA256 digest of the cookie value and retrieving a serialized
Session from an external datastore based on that digest.

### Expiry

In addition to setting an expiry on the session cookie, trillium
sessions include the same expiry in their serialization format. If an
adversary were able to tamper with the expiry of a cookie, trillium
sessions would still check the expiry on the contained session before
using it

### If anything goes wrong with the above process

If there are any failures in the above session retrieval process, a
new empty session is generated for the request, which proceeds through
the application as normal.

## Stale/expired session cleanup

Any session store other than the cookie store will accumulate stale
sessions.  Although the trillium session handler ensures that they
will not be used as valid sessions, For most session stores, it is the
trillium application's responsibility to call cleanup on the session
store if it requires it

```
use trillium::Conn;
use trillium_cookies::{CookiesHandler, cookie::Cookie};
use trillium_sessions::{MemoryStore, SessionConnExt, SessionHandler};
# std::env::set_var("TRILLIUM_SESSION_SECRET", "this is just for testing and you should not do this");
let session_secret = std::env::var("TRILLIUM_SESSION_SECRET").unwrap();

let handler = (
    CookiesHandler::new(),
    SessionHandler::new(MemoryStore::new(), session_secret.as_bytes()),
    |conn: Conn| async move {
        let count: usize = conn.session().get("count").unwrap_or_default();
        conn.with_session("count", count + 1)
            .ok(format!("count: {}", count))
    },
);

use trillium_testing::prelude::*;
let mut conn = get("/").on(&handler);
assert_ok!(&mut conn, "count: 0");

let set_cookie_header = conn.headers_mut().get("set-cookie").unwrap().as_str();
let cookie = Cookie::parse_encoded(set_cookie_header).unwrap();

let make_request = || get("/")
    .with_request_header(("cookie", &*format!("{}={}", cookie.name(), cookie.value())))
    .on(&handler);

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
