# CORS

[rustdocs](https://docs.rs/trillium-cors)

By default a browser will not let a page read a response from a different origin than the page itself. The `trillium-cors` crate is how a server says which origins it will make an exception for, implementing the [Fetch standard's CORS protocol](https://fetch.spec.whatwg.org/#http-cors-protocol).

Mount it ahead of the rest of the application. It does two things: it answers the `OPTIONS` preflight a browser sends before any request that isn't a plain `GET`, `HEAD`, or form `POST`, and it adds the headers that let a page read the response to the requests that follow.

## Setup

```rust
# [dependencies]
# trillium = "1"
# trillium-smol = "0.7"
# trillium-cors = "0.1"
#
use trillium::Method;
use trillium_cors::Cors;

fn main() {
    trillium_smol::run((
        Cors::allow_origins(["https://app.example.com"])
            .allow_methods([Method::Get, Method::Post, Method::Delete])
            .allow_headers(["content-type", "authorization"]),
        "hello from an api",
    ));
}
```

## The browser is the enforcement point

CORS is not a server-side access control. A request from an origin this handler does not allow still runs; what the browser withholds is the page's ability to read the *response*. So a disallowed origin is answered normally, minus the CORS headers, and a request with no `Origin` header at all passes through untouched — which is what keeps non-browser clients working. `reject_disallowed_origins()` trades that for a `403`.

Anything that must actually be denied — authentication, authorization, [CSRF](./csrf.md) — needs a handler that enforces it, whether or not this one is present.

## Origin policies

Build a `Cors` by naming an origin policy, then refine it with the setters.

`Cors::allow_origins` takes a fixed list. An origin is scheme, host, and port, and all three are compared: a request from `http://example.com` does not match an allowed `https://example.com`.

`Cors::allow_origin_fn` takes a predicate over the parsed origin, for policies a list can't express:

```rust
# [dependencies]
# trillium = "1"
# trillium-cors = "0.1"
#
# fn main() {
use trillium_cors::{Cors, Origin};

let cors = Cors::allow_origin_fn(|origin| match origin {
    Origin::Tuple(scheme, host, _) => {
        scheme == "https" && host.to_string().ends_with(".example.com")
    }
    Origin::Opaque(_) => false,
});
# }
```

`Cors::allow_any_origin` sends a literal `*`, making responses readable by any page on the web — only appropriate for genuinely public data.

To let the browser send credentials (cookies, TLS client certificates, `Authorization` headers) with cross-origin requests, add `allow_credentials()`. Credentials cannot be combined with `allow_any_origin` — the constructor panics rather than reflecting arbitrary origins back, which would hand every site on the web an authenticated read of yours.

## Preflight configuration

`allow_methods` names the methods a page may use cross-origin. `GET`, `HEAD`, and `POST` are permitted without being listed, because a browser exempts them from the preflight method check; every other method has to be named.

`allow_headers` names the request headers a page may send. Note that `content-type` needs listing: it is safelisted only for the three form encodings, so a page sending `application/json` needs permission. `allow_any_header()` approves whatever the preflight asks for.

`max_age` sets how long a browser may cache a preflight response; omitting it means a preflight before every request that needs one.

## Exposing response headers

A page can always read the CORS-safelisted response headers (`cache-control`, `content-language`, `content-length`, `content-type`, `expires`, `last-modified`, and `pragma`). `expose_headers` adds to that set:

```rust
# [dependencies]
# trillium = "1"
# trillium-cors = "0.1"
#
# fn main() {
# use trillium_cors::Cors;
let cors = Cors::allow_origins(["https://app.example.com"])
    .expose_headers(["x-request-id", "x-total-count"]);
# }
```

## On the conn

The `CorsConnExt` trait exposes the decision to other handlers: `conn.cors_origin()` returns the request's origin if the policy allowed it, and `conn.is_cors_preflight()` reports whether the request was an approved preflight. Approved preflights are halted before reaching the application, so the `log_formatter` module provides formatters for [`trillium-logger`](./logger.md) that keep them from being mistaken for the requests they were asking about.

## WebSockets

Browsers do not apply CORS to the WebSocket handshake; that check belongs to the server, which has to compare the `Origin` header itself. [`trillium-websockets`](./websockets.md) performs origin checking at upgrade time — adding this handler to an application will not protect a WebSocket endpoint.
