# CSRF Protection

[rustdocs](https://docs.rs/trillium-csrf)

Cross-site request forgery is an attack in which another site causes a user's browser to make a state-changing request to yours, carrying the user's cookies with it. The `trillium-csrf` crate rejects those requests using metadata browsers attach to every request — it needs no tokens, no cookies, and no configuration to protect an app whose frontend and api share an origin.

## Setup

Place it before any handler with side effects:

```rust
# [dependencies]
# trillium = "1"
# trillium-smol = "0.7"
# trillium-csrf = "0.1"
#
use trillium_csrf::csrf;

fn main() {
    trillium_smol::run((
        csrf(),
        |conn: trillium::Conn| async move { conn.ok("hello") },
    ));
}
```

## How it decides

For each request, in order:

- `GET`, `HEAD`, and `OPTIONS` requests are always allowed. Browsers send those methods ambiently — navigations, images, plain forms — so rejecting them cross-origin would break ordinary links to your site. This list is not configurable; anything state-changing belongs on another method anyway.
- If the request has a `Sec-Fetch-Site` header, it is allowed when the value is `same-origin` or `none` (a user-initiated request such as a bookmark or a typed address) and otherwise rejected, unless the `Origin` header is trusted.
- Without `Sec-Fetch-Site` but with an `Origin` header, the request is allowed when the origin's host and port match the request's own host and otherwise rejected, unless the origin is trusted. Schemes are not compared, so this behaves correctly behind a tls-terminating reverse proxy.
- A request with neither header is allowed: it did not come from a browser, so it cannot carry a browser's ambient credentials, and cross-site request forgery does not apply.

Rejections halt the conn with a `403` and log the check that failed along with the configuration that would allow the request if it was legitimate.

## Trusting other origins

If pages on another origin legitimately make state-changing requests to this app, name that origin:

```rust
# [dependencies]
# trillium = "1"
# trillium-csrf = "0.1"
#
# fn main() {
use trillium_csrf::csrf;

let handler = csrf().with_trusted_origins(["https://app.example.com"]);
# }
```

Origins are compared exactly — no wildcards, and subdomains of a trusted origin are not trusted.

## Exempting a route

Webhook endpoints don't need an exemption: webhook senders are not browsers, send neither header, and are allowed. If a route must accept browser requests from origins you can't enumerate — a multi-tenant single-sign-on callback, say — run the handler conditionally by wrapping it:

```rust
# [dependencies]
# trillium = "1"
# trillium-csrf = "0.1"
#
# fn main() {
use trillium::{Conn, Handler};
use trillium_csrf::{Csrf, csrf};

struct ExemptSsoCallback(Csrf);

impl Handler for ExemptSsoCallback {
    async fn run(&self, conn: Conn) -> Conn {
        if conn.path() == "/sso/callback" {
            conn
        } else {
            self.0.run(conn).await
        }
    }
}

let handler = ExemptSsoCallback(csrf());
# }
```

## What this does not cover

Browsers released before roughly 2019 may send neither `Sec-Fetch-Site` nor `Origin` on cross-site form submissions, and this handler allows those requests. Protecting that population requires request tokens, which this crate does not provide. For the reasoning behind header-based protection, see [Cross-Site Request Forgery](https://words.filippo.io/csrf/).

Apis authenticated exclusively by a bearer token or other explicit request header don't need this crate: cross-site request forgery is only possible when authentication is ambient, as with cookies or network position.
