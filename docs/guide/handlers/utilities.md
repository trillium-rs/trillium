# Utility Handlers

These smaller crates handle HTTP-level concerns that most applications encounter. They're each a separate crate so you only compile what you use.

## Compression

[rustdocs](https://docs.trillium.rs/trillium_compression)

Compresses response bodies using zstd, brotli, or gzip (in that preference order), selected based on the client's `Accept-Encoding` header. Place it early in your handler chain so it wraps all downstream responses.

```rust
use trillium_compression::Compression;

trillium_smol::run((
    Compression::new(),
    |conn: trillium::Conn| async move { conn.ok("this body will be compressed") },
));
```

## Caching headers

[rustdocs](https://docs.trillium.rs/trillium_caching_headers)

Handles `ETag`, `Last-Modified`, `If-None-Match`, and `If-Modified-Since` headers. When a downstream handler sets an etag or last-modified timestamp, this handler automatically returns `304 Not Modified` for unchanged resources.

```rust
use trillium_caching_headers::CachingHeaders;

trillium_smol::run((
    CachingHeaders::new(),
    // downstream handlers set ETags or Last-Modified values
));
```

## Conn ID

[rustdocs](https://docs.trillium.rs/trillium_conn_id)

Assigns each request a unique identifier. The ID is accessible via the `ConnIdExt` trait extension on `Conn`, and can be added to log output via a formatter for `trillium-logger`.

```rust
use trillium_conn_id::{ConnId, ConnIdExt};

trillium_smol::run((
    ConnId::new(),
    |conn: trillium::Conn| async move {
        let id = conn.conn_id().to_string();
        conn.ok(format!("request id: {id}"))
    },
));
```

## Head

[rustdocs](https://docs.trillium.rs/trillium_head)

Handles `HEAD` requests by running the full handler chain as if it were a `GET`, then stripping the response body before sending. Without this, `HEAD` requests return a 404 unless a handler explicitly checks for them.

```rust
use trillium_head::Head;

trillium_smol::run((Head::new(), my_handler));
```

## Method override

[rustdocs](https://docs.trillium.rs/trillium_method_override)

Rewrites the HTTP method based on a `_method` query parameter. HTML forms only support `GET` and `POST`; this handler lets them submit `DELETE`, `PATCH`, and `PUT` requests.

```rust
use trillium_method_override::MethodOverride;

trillium_smol::run((MethodOverride::new(), my_handler));
// POST /items/42?_method=DELETE arrives at my_handler as DELETE /items/42
```

## Forwarding

[rustdocs](https://docs.trillium.rs/trillium_forwarding)

When Trillium runs behind a reverse proxy, the remote IP and protocol in `Conn` reflect the proxy, not the actual client. This handler reads `Forwarded` and `X-Forwarded-*` headers from trusted proxies and corrects those values.

```rust
use trillium_forwarding::Forwarding;

trillium_smol::run((
    Forwarding::trust_always(), // or configure trusted IP ranges
    my_handler,
));
```

## Basic auth

[rustdocs](https://docs.trillium.rs/trillium_basic_auth)

Enforces HTTP Basic Authentication. Requests without valid credentials receive a `401 Unauthorized` response with a `WWW-Authenticate: Basic` challenge.

```rust
use trillium_basic_auth::BasicAuth;

trillium_smol::run((
    BasicAuth::new("admin", "s3cr3t").with_realm("My App"),
    |conn: trillium::Conn| async move { conn.ok("authenticated") },
));
```
