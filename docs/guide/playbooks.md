# Playbooks

The handler crates are designed to be picked à la carte, but most applications fall into a few recognizable shapes. These are complete, copy-paste starting points for those shapes — take one, delete what you don't need, and adjust the placeholders (`yourdomain.example`) to your origins.

Order matters, and each tuple below encodes it. Handlers run left to right, and `before_send` hooks run in *reverse* order on the way out — which is why `Logger` goes first: it runs its formatter last, observing the final response, after compression and everything else.

## Server-rendered website

The full batteries-included set for a site that renders HTML, uses session cookies, and serves its own assets:

```rust
# [dependencies]
# trillium = "1"
# trillium-smol = "0.7"
# trillium-logger = "0.5"
# trillium-compression = "0.3"
# trillium-head = "0.3"
# trillium-cookies = "0.5"
# trillium-sessions = "0.6"
# trillium-csrf = "0.1"
# trillium-router = "0.5"
# trillium-static = { version = "0.6", features = ["smol"] }
#
use trillium::Conn;
use trillium_compression::Compression;
use trillium_cookies::CookiesHandler;
use trillium_csrf::Csrf;
use trillium_head::Head;
use trillium_logger::Logger;
use trillium_router::Router;
use trillium_sessions::{MemoryStore, SessionHandler};
use trillium_static::{crate_relative_path, StaticFileHandler};

fn main() {
    trillium_smol::run((
        Logger::new(),
        Compression::new(),
        Head::new(),
        CookiesHandler::new(),
        SessionHandler::new(MemoryStore::new(), "01234567890123456789012345678901123"),
        Csrf::new(),
        Router::new()
            .get("/", |conn: Conn| async move { conn.ok("home") })
            .post("/signup", |conn: Conn| async move { conn.ok("signed up") }),
        StaticFileHandler::new(crate_relative_path!("examples/files")).with_index_file("index.html"),
    ));
}
```

Why this order:

- `Logger` first, so its `before_send` runs last and logs the response as sent.
- `Compression` before anything that sets a body, so its `before_send` wraps them all.
- `Head` before the router, so route handlers see `HEAD` requests as `GET` and never build a body that would be discarded.
- `CookiesHandler` before `SessionHandler`, which stores the session key in a cookie.
- `csrf()` before anything with side effects. It needs no configuration when the site's pages post back to the site itself.
- The static file handler last: it only runs if no route matched.

There's no CORS handler here — a site whose pages talk only to their own origin doesn't need one, and adding one wouldn't make it more secure (see [the browser is the enforcement point](./handlers/cors.md#the-browser-is-the-enforcement-point)).

The `MemoryStore` and the inline session secret are development conveniences: sessions vanish on restart, and a secret in source is a secret leaked. In production, use a persistent [session store](./handlers/sessions.md#session-stores) and load the secret from the environment.

## JSON api with a browser frontend on another origin

A cookie-authenticated api at `api.yourdomain.example` serving a frontend at `app.yourdomain.example`. The frontend's origin shows up twice — CORS lets its pages *read* responses, and CSRF lets its pages *make* state-changing requests:

```rust
# [dependencies]
# trillium = "1"
# trillium-smol = "0.7"
# trillium-logger = "0.5"
# trillium-cookies = "0.5"
# trillium-sessions = "0.6"
# trillium-cors = "0.1"
# trillium-csrf = "0.1"
# trillium-router = "0.5"
#
use trillium::{Conn, KnownHeaderName, Method};
use trillium_cookies::CookiesHandler;
use trillium_cors::Cors;
use trillium_csrf::Csrf;
use trillium_logger::Logger;
use trillium_router::Router;
use trillium_sessions::{MemoryStore, SessionHandler};

const FRONTEND: &str = "https://app.yourdomain.example";

fn main() {
    trillium_smol::run((
        Logger::new(),
        Cors::allow_origins([FRONTEND])
            .allow_credentials()
            .allow_methods([Method::Delete, Method::Put])
            .allow_headers([KnownHeaderName::ContentType]),
        Csrf::new().with_trusted_origins([FRONTEND]),
        CookiesHandler::new(),
        SessionHandler::new(MemoryStore::new(), "01234567890123456789012345678901123"),
        Router::new()
            .get("/widgets", |conn: Conn| async move { conn.ok("[]") })
            .post("/widgets", |conn: Conn| async move { conn.ok("created") }),
    ));
}
```

`allow_credentials` is what lets the browser send the session cookie cross-origin, and it's why the origin must be named rather than wildcarded. `allow_headers([KnownHeaderName::ContentType])` is what permits `application/json` request bodies — `content-type` is only CORS-safelisted for form encodings.

If the api is authenticated by a bearer token instead of cookies, most of this falls away: drop `csrf()`, `CookiesHandler`, `SessionHandler`, and `allow_credentials`, and add `authorization` to `allow_headers`. Cross-site request forgery only applies to ambient credentials like cookies, and without credentials the CORS policy is just naming who may read.

## Variations

- **Behind a reverse proxy or load balancer**: add [`Forwarding`](./handlers/utilities.md#forwarding) at the front of the tuple so `conn.peer_ip()` and the logger report the real client rather than the proxy.
- **Request ids**: add [`ConnId`](./handlers/utilities.md#conn-id) after the logger to tag each request with an identifier usable in log formats and response headers.
- **Conditional `304`s**: add [`CachingHeaders`](./handlers/utilities.md#caching-headers) before the router for `ETag`/`Last-Modified` handling.
- **Single-binary deploys**: swap `trillium-static` for [`trillium-static-compiled`](./handlers/static.md), which embeds the asset directory at compile time.
