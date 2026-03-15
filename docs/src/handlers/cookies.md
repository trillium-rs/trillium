# Cookies

[rustdocs](https://docs.trillium.rs/trillium_cookies)

The `trillium-cookies` crate parses inbound `Cookie` request headers and accumulates outbound `Set-Cookie` response headers. It provides a `CookiesHandler` that must be placed in the handler chain before any handler that reads or writes cookies, and a `CookiesConnExt` trait that extends `Conn` with cookie access methods.

## Setup

```rust,noplaypen
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

```rust,noplaypen
use trillium_cookies::CookiesConnExt;

async fn handler(conn: trillium::Conn) -> trillium::Conn {
    let greeting = if let Some(name) = conn.cookies().get("user_name") {
        format!("welcome back, {}!", name.value())
    } else {
        "hello, stranger!".into()
    };
    conn.ok(greeting)
}
```

## Setting cookies

`conn.with_cookie(cookie)` queues a `Set-Cookie` header for the response. The simplest form takes a `(name, value)` tuple:

```rust,noplaypen
conn.with_cookie(("session_id", "abc123"))
```

For cookies with attributes, use `Cookie::build` from the re-exported `cookie` crate:

```rust,noplaypen
use trillium_cookies::{CookiesConnExt, cookie::Cookie};

let cookie = Cookie::build(("preferences", "theme=dark"))
    .path("/")
    .secure(true)
    .http_only(true)
    .same_site(cookie::SameSite::Strict)
    .build();

conn.with_cookie(cookie)
```

## Removing cookies

To delete a cookie, set it with an empty value and a past expiry:

```rust,noplaypen
use trillium_cookies::cookie::Cookie;
use time::OffsetDateTime;

let expired = Cookie::build("session_id")
    .value("")
    .expires(OffsetDateTime::UNIX_EPOCH)
    .build();

conn.with_cookie(expired)
```

## Full example

```rust,noplaypen
{{#include ../../../cookies/examples/cookies.rs}}
```
