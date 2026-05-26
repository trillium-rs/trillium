# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.9.3] - 2026-05-26

### Added

- `ResponseBody::trailers(&self) -> Option<&Headers>` borrows the trailers received after the
  response body, on both borrowed and owned response bodies. Populated after the body has been
  read to end-of-stream.

### Fixed

- HTTP/2: a received body that carried a `content-length` header could report end-of-input before
  the stream's trailing headers arrived, so `request_trailers()` (server) / `response_trailers()`
  (client) came back empty even though the peer had sent trailers. The effect was a timing-dependent
  race condition. Bodies now wait for the end of the stream before reporting end-of-input, so the
  trailers are always delivered.
- HTTP/2 and HTTP/3 response trailers are now surfaced on the owned response body taken via
  `Conn::take_response_body` (and the `From<Conn> for Body` proxy path). Previously the protocol
  session wasn't carried onto the detached body, so driver-decoded trailers were never harvested,
  and even when present they were discarded by the body's EOF recycle before a caller could read
  them. The borrowed `Conn::response_body` path was unaffected.
- HTTP/2: response trailers could go missing when talking to a server that responds and resets the
  stream before reading the full request body — the body ended cleanly but the trailers the server
  had already sent were dropped. Trailers are now delivered in this case.
- HTTP/2: issuing several requests concurrently through one `Client` to the same origin could abort
  the connection with a protocol error, failing those requests and any others in flight on the same
  connection with a connection-aborted error. Concurrent requests over a shared connection now
  proceed normally.


## [0.9.2] - 2026-05-25

### Changed

- A request body set on a conn marked for upgrade (`ConnExt::upgrade`) is now sent as a
  prelude before the connection transitions to the bidirectional/upgraded stream, across
  HTTP/1.1, HTTP/2, and HTTP/3. Previously the body was silently dropped. `Content-Length`
  is stripped on these requests, since the stream stays open past the prelude.

### Fixed

- HTTP/2: a request could hang forever when the connection closed without delivering a
  response — a graceful `GOAWAY`, a peer FIN, or an I/O error — unless the server had
  explicitly reset that stream. In-flight requests (awaiting response headers, reading a
  response body, or writing to an upgraded stream) now surface a connection-aborted /
  broken-pipe error instead of hanging.
- HTTP/3: reading a response body could hang or fail with a spurious `UnexpectedEof` when
  the body's first DATA frame had been buffered alongside the headers and was then read with
  a buffer smaller than a frame header — as happens reading a body one byte at a time.
  Bodies read with a larger buffer, or whose body arrived separately from the headers, were
  unaffected. These now decode correctly.
- HTTP/1.1: reading a chunked-encoded response body could in rare cases fail to decode
  despite the wire being well-formed, surfacing as one of `chunk header too long`,
  `invalid chunk size`, `ConnectionAborted`, or `UnexpectedEof`. Decode errors are now
  surfaced only for genuinely malformed bodies or actual transport closure.
- HTTP/1.0: responses from servers that omit `Content-Length` (read-to-close framing) now
  decode correctly. Previously these surfaced as chunked-decode errors.

### Security

- HTTP/2: a response whose `content-length` header disagreed with the actual length of its
  body was accepted rather than rejected. A body longer than declared was silently truncated
  at the declared length; a shorter one was only caught if it happened to be read to its end.
  The gap between a declared and actual body length is a response-smuggling / desync
  primitive. Such responses are now rejected with a stream error.

## [0.9.1] - 2026-05-15

### Added

- `Conn::into_websocket` now works over HTTP/3, in addition to the existing HTTP/2 and HTTP/1.1
  support. Hint the conn with `Version::Http3` to upgrade a WebSocket over an HTTP/3 connection.
- `ConnExt::upgrade` and `ConnExt::is_upgrade` — mark a client conn for an upgrade. With `upgrade`
  marked, executing the conn transmits only the request headers and leaves the outbound direction
  open; the conn can then be converted into a `trillium_http::Upgrade<Box<dyn Transport>>` once
  response headers arrive (`Upgrade::from(conn)`).

## [0.9.0] - 2026-05-15

### Added

#### Client middleware: `ClientHandler` and `ConnExt`

`trillium-client` gains an extension point comparable to `trillium::Handler`, with a similar
`Handler` / tuple-composition / `halt`-to-short-circuit shape, adapted to client ownership semantics.

**`ClientHandler` trait.** Two async hooks, both with no-op defaults:

- `run(&self, conn: &mut Conn)` — fires before the network round-trip, in declared order. May mutate
  the request, halt to short-circuit (cache hit, mocked response), or fail.
- `after_response(&self, conn: &mut Conn)` — fires after the network round-trip in *reverse*
  declared order, *always* (including on transport error and on halt-and-synthesize). Observer
  handlers see every response; recovery handlers can clear a stashed transport error and synthesize
  a fallback.

Tuples up to 15 elements implement `ClientHandler`, as does `()` and `Option<H>`. Implementors write
the trait with native `async fn`.

**Installation and recovery.** `Client::with_handler` / `Client::set_handler` install a handler on a
client, and `Client::downcast_handler` recovers the concrete type for inspection (counters on a
metrics handler, etc.).

**`ConnExt` extension trait.** The lifecycle-driving methods — queueing a follow-up request,
stashing/recovering the transport error, halting, and the response-state synthesis surface
(`set_status`, `set_response_body`, `response_headers_mut`, etc.) — live on a separate extension
trait that handler authors bring into scope with `use trillium_client::ConnExt;`. These
operations are intended only for use from inside a handler and do not appear on `Conn`'s inherent
surface.

**Re-issuing requests from a handler.** Awaiting a `Conn` may now execute an unbounded chain of
follow-up requests within a single await. Handlers that re-issue (following redirects, retry logic,
auth-refresh, etc.) build a fresh `Conn` from `conn.client()`, configure it, and queue it via
`ConnExt::set_followup` from inside `after_response`; once the current cycle's
`after_response` fully unwinds, the current response body is recycled, the follow-up is swapped into
place, and another full handler cycle runs on it. The `Conn` left in place when the await resolves
may therefore be a follow-up rather than the one the caller started with.

#### Override response bodies and the `ResponseBody` lifecycle

Supporting infrastructure for the ability for handlers to synthesize a response body without hitting
the network, for use cases such as cache-populated responses:

- `ConnExt::set_response_body` / `with_response_body` — install an
  override body that bypasses the transport. Accepts anything convertible to
  a `Body`; `Content-Length` / `Transfer-Encoding` are reconciled to the
  body's length, and the user-set `max_len` is enforced for override bodies
  as well as transport-backed ones.
- `Conn::take_response_body(&mut self) -> Option<ResponseBody<'static>>` —
  detaches the body so the caller can wrap, replace, drain, or hold it.
  Returns `None` on the second call.
- `ResponseBody::recycle` (consuming, async) — drains the body and returns
  the connection to the pool when reuse is possible, otherwise closes it.
- `Drop for ResponseBody<'static>` — the same drain-and-recycle-or-close
  runs when a `ResponseBody` is dropped without being explicitly recycled.

#### Other additions

- `Conn::client() -> &Client` — handlers building a follow-up conn use this
  to reach the originating client.
- `Conn::request_body() -> Option<&Body>` — request-body accessor.
- `pub use url` — the `url` crate is re-exported at the crate root, so
  callers don't need to depend on `url` separately to write `IntoUrl` impls
  or inspect a `Conn::url()`.

### Removed

- `From<ResponseBody<'a>> for ReceivedBody<'a, _>` and
  `From<Conn> for ReceivedBody<'static, _>` — the two are no longer
  directly interchangeable.
- `AsyncRead::poll_read_vectored for ResponseBody` — the override path has
  no meaningful vectored read; the default (single-buffer) impl applies
  uniformly.

### Fixed

- `InvalidStatus` is now propagated as an error from the awaited conn
  instead of panicking when the server returns an unrecognized response
  code.

## [0.8.4] - 2026-05-11

### Fixed

- HTTP/1.1, HTTP/2, HTTP/3: interim 1xx responses (early hints / RFC 8297, and any other informational status that isn't `100 Continue` or `101 Switching Protocols`) are now correctly skipped and their headers discarded rather than merged into the final response, per RFC 9110 §15.2 and RFC 8297 §2. While awaiting `100 Continue` before sending a request body, an unrelated interim status is now skipped instead of suppressing the body.
- HTTP/1.1: a response whose `Transfer-Encoding` last coding is not `chunked` is now rejected as `Error::UnexpectedHeader(TransferEncoding)` rather than being parsed as chunked over raw bytes. Per RFC 9112 §6.3 the framing is ambiguous in that case; the previous behavior left a connection in a state where pool reuse would be a response-smuggling vector.

## [0.8.3] - 2026-05-07

### Fixed

- HTTP/3: bidi streams now `RESET_STREAM` on stream-level protocol errors (RFC 9114 §4.1.2), and uni streams fire `CONNECTION_CLOSE` before the recv stream drops to avoid a `FINAL_SIZE_ERROR` race. Adopts `H3Connection::process_inbound_bidi_with_reset` / `process_inbound_uni_with_close` from `trillium-http` 1.2.

## [0.8.2] - 2026-05-06

### Fixed

- Update `trillium-webtransport` dependency to the latest.

## [0.8.1] - 2026-05-05

### Fixed
- Bump intra-crate dependency version specifiers (`trillium-server-common`, `trillium-websockets`) to match the 1.1 release; `0.8.0` was published with stale specs.

## [0.8.0] - 2026-05-05 [YANKED]

### Added

#### HTTP/2

The client now speaks HTTP/2. With a TLS connector that negotiates `h2` via ALPN, h2 is used automatically; for cleartext h2c, set `Conn::with_http_version(Version::Http2)` for prior-knowledge dispatch. h2 connections are pooled and multiplex concurrent requests over a single connection.

```rust
use trillium_client::Client;
use trillium_rustls::RustlsConfig;

let client = Client::new(RustlsConfig::default()); // h2 advertised in ALPN automatically
```

- `Client::with_h2_idle_timeout` / `with_h2_idle_ping_threshold` / `with_h2_idle_ping_timeout` (and `set_*` / `without_*` variants) — h2 connection idle and health-check tuning
- `Conn::protocol() -> Option<&str>` — the negotiated protocol (`Some("h2")` once the response has been received over h2)

#### WebTransport client (RFC 9220 + draft-ietf-webtrans-http3)

Behind the new `webtransport` cargo feature, `Client::webtransport(url)` builds a [`Conn`] preconfigured for an extended-CONNECT WebTransport handshake (method=CONNECT, `:protocol = webtransport`, http_version=HTTP/3). Set headers as usual, then call `Conn::into_webtransport().await` to complete the upgrade and obtain a [`WebTransportConnection`](trillium_webtransport::WebTransportConnection) — the same type the server-side handler receives, so any code that handles streams + datagrams works on either side.

Multiple sessions to the same origin coalesce onto a single underlying QUIC connection. Each `into_webtransport` call opens a new bidi stream for the CONNECT and registers a new session with the connection's per-origin router, mirroring how HTTP/3 request multiplexing already worked. Most servers don't coalesce on the client side; this client does.

```rust,ignore
let client = trillium_client::Client::new_with_quic(connector, quic);
let wt = client
    .webtransport("https://example.com/echo")
    .with_request_header("origin", "https://example.com")
    .into_webtransport()
    .await?;

let mut stream = wt.open_bidi().await?;
stream.write_all(b"hi").await?;
stream.close().await?;
```

The peer-capability gate (RFC 9220 §3 — server must advertise `SETTINGS_ENABLE_CONNECT_PROTOCOL`, `SETTINGS_ENABLE_WEBTRANSPORT`, and `SETTINGS_H3_DATAGRAM`) is enforced before any HEADERS go on the wire; if the server doesn't advertise the required settings the upgrade fails with `ErrorKind::ExtendedConnectUnsupported` rather than getting stuck.

#### `:protocol` pseudo-header for HTTP/3 extended CONNECT

The HTTP/3 send path now honors `Conn::protocol`, mirroring the HTTP/2 extended-CONNECT path added in this release cycle. Required for WebTransport-over-h3; available for any future protocol that bootstraps over extended CONNECT (RFC 9220).

#### Other additions

- `Client::without_timeout()` — builder counterpart to `with_timeout`
- `BodySource` re-exported from `trillium-http`

## [0.7.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0
- `ObjectSafeConnector` replaced by `ArcedConnector`; `config.arced()` → `ArcedConnector::new(config)`
- Error variants renamed: `MalformedHeader` split into `InvalidHeaderName` and `InvalidHeaderValue`; `PartialHead` merged into `InvalidHead`
- Maximum head size increased from 2KB to 8KB
- Previously deprecated `with_header`, `with_headers`, and `without_header` removed
- `async_trait` re-export removed
- `Client::with_default_pool` removed and keepalive is now the default. To opt out, `Client::without_keepalive` was added
- `Conn::response_body` now returns a `trillium_client::ResponseBody` instead of a `trillium_http::ReceivedBody`

### Added

#### HTTP/3

`Client::new_with_quic(connector, quic_connector)` builds a client with HTTP/3 support. The client tracks `Alt-Svc` response headers and automatically uses HTTP/3 for subsequent requests to origins that advertise it. QUIC connections are pooled; if an H3 attempt fails, that endpoint is marked broken and requests fall back to HTTP/1.1 for a backoff period before retrying. Requests to origins without a cached alt-svc entry always use HTTP/1.1.

`QuicClientConfig` and `ArcedQuicClientConfig` are re-exported from `trillium-server-common`. The `QuicClientConfig` type parameter is bound at construction time (before type erasure), keeping `trillium-quinn` and the runtime adapter as independent crates that neither depends on the other.

```rust
use trillium_client::Client;
use trillium_rustls::RustlsConfig;
use trillium_rustls::rustls::client::ClientConfig;
use trillium_quinn::ClientQuicConfig;

let client = Client::new_with_quic(
    RustlsConfig::<ClientConfig>::default(),
    ClientQuicConfig::with_webpki_roots(),
);
```

#### Other additions

- `Conn::http_version() -> Version` — returns the HTTP version used for the request; after the request completes this reflects whether HTTP/3 was negotiated
- `Client::with_timeout(Duration)` and `Conn::with_timeout(Duration)` — per-request timeouts, returning `Error::TimedOut` on expiry
- `Client::set_timeout(&mut self, Duration)` and `Conn::set_timeout(&mut self, Duration)` — in-place variants of the above
- Per-connection state via `TypeSet`: `with_state`, `insert_state`, `state`, `state_mut`, `take_state`
- `sonic-rs` feature: opt-in alternative to `serde_json` for `with_json_body` and `response_json`. Enable with `features = ["sonic-rs"]`. The two features are mutually exclusive — enable only one. **Note:** unlike `serde_json`, `sonic-rs` does not guarantee stable map key ordering — tests that assert on raw JSON string output may need to parse back to `Value` before comparing. To keep using `serde_json`, use `features = ["serde_json"]`.

### Added
- deprecate Headers::contains_ignore_ascii_case
- *(client)* impl IntoUrl for IpAddr and SocketAddr for convenience

## [0.6.1](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.6.0...trillium-client-v0.6.1) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Fixed
- *(client)* re-add Conn::without_header

## [0.6.0](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.5.6...trillium-client-v0.6.0) - 2024-04-04

### Fixed
- *(client)* [**breaking**] client use of server-common 0.5 was a breaking change

### Other
- release
- release
- release
- clippy
- *(client)* remove references to `with_websocket_upgrade_headers`

## [0.5.6](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.5.5...trillium-client-v0.5.6) - 2024-02-13

### Added
- *(http)* sort Host and Date headers first

### Fixed
- *(client)* set minimum trillium-http version correctly

## [0.5.5](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.5.4...trillium-client-v0.5.5) - 2024-02-05

### Added
- *(client)* fix feature specification

## [0.5.4](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.5.3...trillium-client-v0.5.4) - 2024-01-24

### Fixed
- *(security)* handling of unsafe characters in outbound header names and values

### Other
- *(deps)* update env_logger requirement from 0.10.1 to 0.11.0

## [0.5.3](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.5.2...trillium-client-v0.5.3) - 2024-01-22

### Other
- Make `into_websocket()` send the request if not yet sent
- Rename `websocket` feature to `websockets`
- Add client WebSocket support

## [0.5.2](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.5.1...trillium-client-v0.5.2) - 2024-01-10

### Added
- *(client)* reexport trillium_http::{Body, Method}
- *(client)* reexport ObjectSafeConnector
- *(client)* add Client::connector to borrow the connector
- *(client)* add IntoUrl impls for slices, arrays and vecs

### Other
- Release only rustls
- release
- *(client)* construct Conn directly in Client::build_conn

## [0.5.1](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.5.0...trillium-client-v0.5.1) - 2024-01-02

### Other
- Add tests for using `String` with `IntoUrl`
- `impl IntoUrl for String` for convenience
- use #[test(harness)] instead of #[test(harness = harness)]
- Update test-harness requirement from 0.1.1 to 0.2.0

## [0.5.0](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.4.9...trillium-client-v0.5.0) - 2024-01-02

### Other
- update dependencies other than trillium-rustls
- replace insert_default_header and remove_default_header with default_headers_mut and add default_headers
- client breaking: add default headers and make header access consistent
- introduce IntoUrl and Client::base
- remove ClientLike
- http patch reversion: set Server header before request again
- Avoid an unnecessary to_string() before formatting
- update tests
- client patch: spec compliance improvements
- client patch feature: add Conn::peer_addr
