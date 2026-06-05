# Client middleware

Requests made by a `Client` can run through a stack of middleware. A client handler can rewrite the outbound request, observe or rewrite the response, short-circuit the network entirely (a cache hit or a mock), or re-issue the request (following a redirect, retrying, refreshing a token).

The interface is `ClientHandler`. It mirrors the server-side `Handler`: handlers compose into tuples, run left-to-right, and a handler can halt the conn to stop the chain. Because a client conn is owned by the caller rather than the framework, handlers take `&mut Conn` and return `Result<()>` — client work can fail outright (a TLS handshake, a signing step) in a way a server handler can't.

Install a stack with `Client::with_handler`:

```rust
# [dependencies]
# trillium-client = { path = "../client" }
# trillium-testing = { path = "../testing" }
# trillium-logger = { path = "../logger", features = ["client"] }
# trillium-cookies = { path = "../cookies", features = ["client"] }
# trillium-compression = { path = "../compression", features = ["client"] }
# trillium-redirect = { path = "../redirect", features = ["client"] }
#
# fn main() {
use trillium_client::Client;
use trillium_testing::client_config;
use trillium_logger::client::ClientLogger;
use trillium_cookies::client::Cookies;
use trillium_compression::client::Compression;
use trillium_redirect::client::FollowRedirects;

let client = Client::new(client_config()).with_handler((
    ClientLogger::new(),
    Cookies::new(),
    Compression::new(),
    FollowRedirects::new(),
));
# }
```

## Lifecycle and ordering

Awaiting a conn runs the stack in three steps:

1. **Forward pass** (`run`) — each handler runs in declared order. A handler may mutate the request, or halt the conn and populate a synthetic response (a cache hit, a mock). If any handler halts, the remaining `run` methods and the network round-trip are skipped.
2. **Network round-trip** — skipped if the conn was halted.
3. **Reverse pass** (`after_response`) — each handler's `after_response` runs in *reverse* declared order, regardless of whether the conn halted or the transport errored. This is where handlers observe the response, including synthesized and failed ones.

Declared order therefore matters, and the two natural anchors are the ends of the tuple:

- A **logger** goes first. Its timing spans only the handlers that run after it, so first position measures the whole stack including the network.
- A **cache** goes last. As the last `run`, a fresh hit short-circuits everything after it; as the first `after_response`, it sees the response body before any other handler consumes the one-shot network stream.

Writing your own handler is an advanced topic — see the [`ClientHandler`](https://docs.trillium.rs/trillium_client/trait.ClientHandler.html) and [`ConnExt`](https://docs.trillium.rs/trillium_client/trait.ConnExt.html) rustdocs, which cover halting, response synthesis, and queuing a follow-up request.

## Logging

`trillium_logger::client::ClientLogger` emits one log line per request. A line is written for every outcome — a successful response, a response synthesized upstream (cache hit, mock), and transport failures like a refused connection or TLS error — so the log reflects everything the client attempted.

The format is built from composable pieces in the `formatters` submodule; the default is `dev_formatter`.

```rust
# [dependencies]
# trillium-client = { path = "../client" }
# trillium-testing = { path = "../testing" }
# trillium-logger = { path = "../logger", features = ["client"] }
#
# fn main() {
use trillium_client::Client;
use trillium_testing::client_config;
use trillium_logger::client::{ClientLogger, formatters};

let client = Client::new(client_config()).with_handler(
    ClientLogger::new().with_formatter((
        formatters::method,
        " ",
        formatters::url,
        " -> ",
        formatters::status,
        " ",
        formatters::response_time,
    )),
);
# }
```

Formatter building blocks include `method`, `url`, `status`, `version`, `secure`, `response_time`, `body_len_human`, `bytes`, `timestamp`, `error`, and `request_header(name)` / `response_header(name)`. `with_color_mode` and `with_target` control colorization and where lines are written, mirroring the server logger.

## Cookies

`trillium_cookies::client::Cookies` maintains an RFC 6265 cookie jar across every request issued by one `Client`. It attaches matching cookies to outbound requests via the `Cookie` header and stores `Set-Cookie` responses, with domain, path, public-suffix, expiry, and `Secure` rules handled by `cookie_store::CookieStore`.

```rust
# [dependencies]
# trillium-client = { path = "../client" }
# trillium-testing = { path = "../testing" }
# trillium-cookies = { path = "../cookies", features = ["client"] }
#
# fn main() {
use trillium_client::Client;
use trillium_testing::client_config;
use trillium_cookies::client::Cookies;

let client = Client::new(client_config()).with_handler(Cookies::new());
# }
```

Seed the jar from an existing store with `Cookies::with_store`, and read it back (to clone or serialize) with `Cookies::borrow`.

## Compression

`trillium_compression::client::Compression` handles content-coding in both directions. It always advertises the codings it can decode via `Accept-Encoding` (unless the caller already set one) and transparently decodes a `Content-Encoding` response it understands, stripping the header so the caller reads plaintext. Brotli, gzip, and zstd are supported.

Compressing the *request* body is opt-in, because HTTP has no pre-request negotiation — sending a compressed body asserts the origin will accept it. Select an encoding handler-wide with `with_default_encoding`, or per request by putting a `CompressionAlgorithm` in the conn's state; the per-request signal overrides the default, and `CompressionAlgorithm::Identity` opts a single request out.

```rust
# [dependencies]
# trillium-client = { path = "../client" }
# trillium-testing = { path = "../testing" }
# trillium-compression = { path = "../compression", features = ["client"] }
#
# fn main() {
use trillium_client::Client;
use trillium_testing::client_config;
use trillium_compression::{client::Compression, CompressionAlgorithm};

// Decode responses, and gzip request bodies by default.
let client = Client::new(client_config())
    .with_handler(Compression::new().with_default_encoding(CompressionAlgorithm::Gzip));
# }
```

## Following redirects

`trillium_redirect::client::FollowRedirects` follows 301, 302, 303, 307, and 308 responses, re-issuing the request through the same client so the connector and pool are reused.

The defaults are conservative:

- **Up to 10 redirects**, then `RedirectError::TooMany`. Change with `with_max_redirects`.
- **HTTPS → HTTP downgrades are blocked.** Allow with `with_allow_downgrade(true)`.
- **Method and body** follow the status: 303 always becomes a bodyless GET; 301/302 turn POST into GET and drop the body; 307/308 keep the method and replay the body if it's a static (cloneable) body. Streaming bodies are one-shot and aren't replayed.
- **Cross-origin redirects drop `Authorization`, `Cookie`, and `Proxy-Authorization`** to avoid leaking credentials. Restrict the allowed targets entirely with `with_allowed_origins`.

```rust
# [dependencies]
# trillium-client = { path = "../client" }
# trillium-testing = { path = "../testing" }
# trillium-redirect = { path = "../redirect", features = ["client"] }
#
# fn main() {
use trillium_client::Client;
use trillium_testing::client_config;
use trillium_redirect::client::FollowRedirects;

let client = Client::new(client_config())
    .with_handler(FollowRedirects::new().with_max_redirects(5));
# }
```

## Retrying

`trillium_client_retry::RetryHandler` re-issues a request that failed in a way worth retrying — a transport error (refused connection, reset, timeout) or a retryable status (429 and 503 by default) — spacing attempts with a backoff schedule and honoring a server's `Retry-After`.

Each attempt is a full client-handler cycle re-queued through the same client, so a logger, conn-id, or metrics handler observes every attempt. Its position in the tuple doesn't much matter — what matters is that it's the only handler queuing follow-ups or handling transport errors, since two handlers each re-driving the conn would compound confusingly.

```rust
# [dependencies]
# trillium-client = "0.9"
# trillium-testing = "0.10"
# trillium-logger = { version = "0.5", features = ["client"] }
# trillium-client-retry = "0.0.1"
#
# fn main() {
use trillium_client::Client;
use trillium_testing::client_config;
use trillium_logger::client::ClientLogger;
use trillium_client_retry::RetryHandler;

let client = Client::new(client_config()).with_handler((
    RetryHandler::new(),
    ClientLogger::new(),
));
# }
```

The defaults are conservative:

- **Idempotent methods only** (GET, HEAD, PUT, DELETE, OPTIONS, TRACE), since replaying a POST risks a duplicate side effect. Opt in to all methods with `with_all_methods` when the endpoint is safe to replay (idempotent in practice, or guarded by an idempotency key).
- **Static bodies only.** A request body is replayed only if it can be cloned (`Vec<u8>`, `String`, `&'static str`, …). A streaming one-shot body can't be replayed, so such a request is surfaced as-is rather than retried.
- **Bounded by attempts and wall-clock.** Retrying stops at `with_max_attempts` total attempts (default 4 — the original plus 3) or the `with_max_elapsed` budget (default 30s), whichever comes first. The budget is a hard ceiling: each retry's timeout is clamped to the time remaining.
- **Full jitter.** The actual delay is chosen uniformly from `0..=computed` to spread retries from many clients across time; turn it off with `without_jitter`.
- **`Retry-After` honored.** A server's delta-seconds `Retry-After` overrides the computed backoff (capped by `with_max_retry_after` and the elapsed budget). HTTP-date values aren't yet parsed and fall back to the computed backoff.

The backoff curve and limits are configurable:

```rust
# [dependencies]
# trillium-client = "0.9"
# trillium-testing = "0.10"
# trillium-client-retry = "0.0.1"
#
# fn main() {
use std::time::Duration;
use trillium_client::Client;
use trillium_testing::client_config;
use trillium_client_retry::RetryHandler;

let client = Client::new(client_config()).with_handler(
    RetryHandler::new()
        .with_exponential_backoff(Duration::from_millis(100))
        .with_max_attempts(5)
        .with_max_elapsed(Duration::from_secs(10)),
);
# }
```

`with_constant_backoff`, `with_linear_backoff`, and `with_custom_backoff` select other curves; `with_max_delay` caps the computed delay. To change *what* gets retried, `with_statuses` replaces the retryable status set and `with_transport_errors` toggles transport-error retries, or replace the whole decision with `retry_when` (a predicate over the conn) or `with_decision` (predicate plus backoff).

## Caching

`trillium_cache::client::Cache` is an RFC 9111 HTTP cache. On a miss it streams the origin response to the caller and into storage simultaneously, so caching doesn't add a buffering hop; on a fresh hit it serves from storage without touching the network. It needs a `CacheStorage` backend — `InMemoryStorage` is built in.

```rust
# [dependencies]
# trillium-client = "0.9"
# trillium-testing = "0.10"
# trillium-cache = { version = "0.1", features = ["client"] }
#
# fn main() {
use trillium_client::Client;
use trillium_testing::client_config;
use trillium_cache::{client::Cache, InMemoryStorage};

let client = Client::new(client_config())
    .with_handler(Cache::new(InMemoryStorage::new()));
# }
```

Mark a shared (proxy/CDN) cache with `.shared()`, cap stored body size with `.with_max_cacheable_size`, and tune the in-memory backend's byte cap and eviction with `InMemoryStorage::with_max_capacity_bytes`, `with_time_to_idle`, and `with_time_to_live`.

As noted under [ordering](#lifecycle-and-ordering), `Cache` goes **last** in the tuple — after the redirect follower and compression so it caches final, decoded responses:

```rust
# [dependencies]
# trillium-client = "0.9"
# trillium-testing = "0.10"
# trillium-logger = { version = "0.5", features = ["client"] }
# trillium-redirect = { version = "0.3", features = ["client"] }
# trillium-cache = { version = "0.1", features = ["client"] }
#
# fn main() {
use trillium_client::Client;
use trillium_testing::client_config;
use trillium_logger::client::ClientLogger;
use trillium_redirect::client::FollowRedirects;
use trillium_cache::{client::Cache, InMemoryStorage};

let client = Client::new(client_config()).with_handler((
    ClientLogger::new(),
    FollowRedirects::new(),
    Cache::new(InMemoryStorage::new()),
));
# }
```
