# Cookies

[rustdocs](https://docs.trillium.rs/trillium_cookies)

The `trillium-cookies` crate parses inbound `Cookie` request headers and accumulates outbound `Set-Cookie` response headers. It provides a `CookiesHandler` that must be placed in the handler chain before any handler that reads or writes cookies, and a `CookiesConnExt` trait that extends `Conn` with cookie access methods.

## Setup

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-cookies = { path = "../cookies" }
#
use trillium_cookies::{CookiesConnExt, CookiesHandler};

fn main() {
    trillium_smol::run((
        CookiesHandler::new(),
        |conn: trillium::Conn| async move {
            // cookies are now available
            conn.ok("handled")
        },
    ));
}
```

> ❗ `CookiesHandler` must come before any handler that calls `conn.cookies()` or `conn.with_cookie()`. If the session handler is also in use, it must come after `CookiesHandler`.

## Reading cookies

`conn.cookies()` returns a reference to the `CookieJar` for the current request:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-cookies = { path = "../cookies" }
#
use trillium_cookies::CookiesConnExt;

# fn main() {
async fn handler(conn: trillium::Conn) -> trillium::Conn {
    let greeting = if let Some(name) = conn.cookies().get("user_name") {
        format!("welcome back, {}!", name.value())
    } else {
        "hello, stranger!".into()
    };
    conn.ok(greeting)
}
# trillium_smol::run(handler);
# }
```

## Setting cookies

`conn.with_cookie(cookie)` queues a `Set-Cookie` header for the response. The simplest form takes a `(name, value)` tuple:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-cookies = { path = "../cookies" }
#
use trillium_cookies::CookiesConnExt;
// ...
# fn main() {
#     trillium_smol::run(|conn: trillium::Conn| async move {
conn.with_cookie(("session_id", "abc123"))
#     });
# }
```

For cookies with attributes, use `Cookie::build` from the re-exported `cookie` crate:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-cookies = { path = "../cookies" }
#
# fn main() {
#     trillium_smol::run(|conn: trillium::Conn| async move {
use trillium_cookies::{CookiesConnExt, cookie::{Cookie, SameSite}};

let cookie = Cookie::build(("preferences", "theme=dark"))
    .path("/")
    .secure(true)
    .http_only(true)
    .same_site(SameSite::Strict)
    .build();

conn.with_cookie(cookie)
#     });
# }
```

## Removing cookies

To delete a cookie, build a removal cookie.

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-cookies = { path = "../cookies" }
#
# use trillium_cookies::{CookiesConnExt, cookie::Cookie};
# fn main() {
#     trillium_smol::run(|conn: trillium::Conn| async move {
conn.with_cookie(Cookie::build("session_id").removal().build())
#     });
# }
```

## Full example

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-cookies = { path = "../cookies" }
# env_logger = "*"
#
use trillium::Conn;
use trillium_cookies::{CookiesConnExt, CookiesHandler};

pub fn main() {
    env_logger::init();

    trillium_smol::run((CookiesHandler::new(), |conn: Conn| async move {
        if let Some(cookie_value) = conn.cookies().get("some_cookie") {
            println!("current cookie value: {}", cookie_value.value());
        }

        conn.with_cookie(("some_cookie", "some-cookie-value"))
            .ok("ok!")
    }));
}
```
