# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.9.11] - 2026-07-06

### Changed

- Performance: sending a request body over HTTP/1.x or HTTP/3 no longer allocates an
  intermediary buffer or copies body content through it.

### Added
- `Client::h1_idle_timeout` (default 5 minutes). A pooled HTTP/1.1 keepalive connection that sits
  idle longer than this is closed and dropped from the pool by a background reaper. Set it to
  `None` to disable expiry and keep the previous retain-until-reused behavior. Configure via
  `with_h1_idle_timeout` / `set_h1_idle_timeout` / `without_h1_idle_timeout`.

- `Client::h3_idle_timeout` (default 5 minutes). A pooled HTTP/3 connection is dropped this long
  after it is established — requests in flight on it are unaffected, and the next request to its
  origin opens a fresh connection. Previously pooled HTTP/3 connections had no expiry at all. Set
  it to `None` to disable. Configure via `with_h3_idle_timeout` / `set_h3_idle_timeout` /
  `without_h3_idle_timeout`.

### Fixed
- A server-initiated bidirectional stream carrying an HTTP request is now treated as the
  connection error RFC 9114 defines it to be: the client closes the connection with
  `H3_STREAM_CREATION_ERROR` and drops it from the pool. Previously the client answered such
  streams with a 404 response and kept the connection pooled. Connection-level protocol errors
  on other inbound bidirectional streams now also close the connection and evict it, where
  previously they were only logged.

- An HTTP/3 connection whose setup failed while opening the mandatory control or QPACK streams
  was pooled as available anyway, and every request to that origin then failed until the
  connection was marked broken. Such a connection is now immediately marked dead and evicted.

- A request body constructed with both a known length and trailers previously wrote the trailer
  lines to the wire after the `Content-Length`-framed body, where the server would read them as
  the start of the next request. Trailers are now sent only with chunked transfer encoding.

- Idle pooled HTTP/1.1 connections to an origin that stopped being contacted were held open
  indefinitely — their file descriptors were released only when the connection was next reused or
  the pool was manually cleaned up. They are now released after `h1_idle_timeout` (default 5
  minutes) even when the origin is never contacted again.

- Expired pooled HTTP/2 and HTTP/3 connections to an origin that stopped being contacted are now
  also reclaimed by the background reaper. These connections pool through a cold-start coalescing
  placeholder, and the resolved placeholder retained a clone of the connection that kept it alive
  past its expiry until the origin was next contacted.

- An HTTP/3 request whose header fields exceed the server's advertised
  `SETTINGS_MAX_FIELD_SECTION_SIZE` is now rejected before being sent (once the server's settings
  are known). Previously the advertised limit was ignored and the request was sent regardless.

## [0.9.10] - 2026-07-05

### Fixed
- A request short-circuited by a `ClientHandler` that halts before the request is sent no longer
  retains its unsent request body. The body was previously held until the conn was dropped, so a
  streaming body fed by a producer that blocks until its bytes are read would hang; a halted conn
  now releases its request body immediately.

## [0.9.9] - 2026-07-04

### Added
- `Client::max_buffered_request_body` (default 1 KiB). A request body of unknown length at or
  below this size is buffered before the request head is written, then sent with an accurate
  `Content-Length` in a single shot. Larger or unbounded bodies stream as
  `Transfer-Encoding: chunked`.
- `Client::expect_continue_timeout` (default 1 second). When a request is sent with
  `Expect: 100-continue`, the client now waits at most this long for the `100 (Continue)` interim
  response before sending the body anyway, rather than waiting indefinitely. This prevents a
  deadlock against a peer that never sends `100 (Continue)` — e.g. an HTTP/1.0 intermediary that
  cannot forward it (RFC 9110 §10.1.1).

### Changed
- `Expect: 100-continue` is no longer sent for every body-carrying request. It is now used only
  when a body exceeds `max_buffered_request_body` (streaming bodies that overflow the buffer, or
  known-length bodies larger than it) — a body small enough to buffer is cheaper to send outright
  than to negotiate. Empty bodies never trigger it (unchanged). This removes a full round-trip from
  small body-carrying requests.

## [0.9.8] - 2026-07-01

### Fixed
- **Security:** a relative path resolved against a client base url can no longer target a host other
  than the base's. A colon in the first path segment (e.g. `/trillium::Handler`) or an embedded
  absolute url (e.g. `/https://elsewhere.example`) was previously resolved as an absolute url —
  sending the request to an attacker-controlled host in the embedded-url case, and failing to resolve
  otherwise. Such inputs are now always joined as paths against the base.
- `Client::build_conn` no longer panics on a malformed url; the error is deferred and surfaced when
  the connection is executed.

## [0.9.7] - 2026-06-23

### Added

- `sse` cargo feature: `Conn::into_sse` executes a request and interprets the response body as a
  `text/event-stream`, returning an `EventStream` — a `Stream` of `Event`s parsed per the
  Server-Sent Events specification (multi-line `data`, `event`, `id`, `retry`, and comments;
  CR/LF/CRLF terminators). This is a single-response stream that ends when the connection closes;
  it does not implement `EventSource`-style automatic reconnection. On failure, `into_sse` returns
  an `SseError` that dereferences to the `Conn` (and converts back via `From`) so the response can
  be inspected. The feature pulls in no new external dependencies.

## [0.9.6] - 2026-06-16

### Added

- `hickory` cargo feature: route all of the client's DNS through an encrypted resolver instead of
  the operating system's. Four resolver configurators select the transport: `Client::with_doh`
  (DNS-over-HTTPS, [RFC 8484]), `Client::with_doh3` (DNS-over-HTTPS reaching the resolver itself over
  HTTP/3, for resolvers that serve it without advertising via Alt-Svc), `Client::with_dot`
  (DNS-over-TLS, [RFC 7858]), and `Client::with_doq` (DNS-over-QUIC, [RFC 9250]). A client routes DNS
  through at most one resolver; a later configurator replaces an earlier one. Resolution is
  fail-closed: a lookup the resolver can't answer fails the request rather than falling back to the
  system resolver, so a query never leaks to a local resolver. One resolution is cached and shared
  across HTTP/1, HTTP/2, and HTTP/3. SVCB/HTTPS records ([RFC 9460]) are honored, so a domain
  advertising `alpn=h3` is reached over HTTP/3 on the first request by an HTTP/3-capable client
  (`Client::new_with_quic`), with no Alt-Svc round-trip; SVCB address hints are dialed on the
  binding's own `port` SvcParam when it specifies one. A request to an IP-literal host (e.g.
  `https://192.0.2.1/`) bypasses the resolver entirely — there is nothing to look up and no SVCB
  records exist for a bare address — so it connects directly rather than failing closed. An
  unreachable resolver — or one that doesn't speak the configured transport, such as a DoT host
  addressed over DoQ — fails the lookup with a descriptive error rather than stalling. `with_dot`
  requires a TLS connector; `with_doh3` and `with_doq` require an HTTP/3-capable client.

[RFC 8484]: https://www.rfc-editor.org/rfc/rfc8484
[RFC 7858]: https://www.rfc-editor.org/rfc/rfc7858
[RFC 9250]: https://www.rfc-editor.org/rfc/rfc9250
[RFC 9460]: https://www.rfc-editor.org/rfc/rfc9460

- Per-request HTTP-version pinning. The `http_version` hint now distinguishes "no hint" from an
  explicit version: an unset hint opts into auto-discovery (Alt-Svc h3, ALPN/pooled h2) as before,
  while setting **any** explicit version — including `Version::Http1_1` — pins that protocol and
  suppresses auto-discovery. A pin also constrains the connection's ALPN so it's honored over TLS:
  an h1 pin advertises only `http/1.1` (a server that would otherwise negotiate `h2` falls back to
  h1), an h2 pin only `h2`. This makes `with_http_version(Version::Http1_1)` the per-request
  equivalent of curl's `--http1.1` — forcing HTTP/1.1 for a single request without disabling h2 on
  the whole client. (`trillium-native-tls` does not yet honor per-connection ALPN, so over it a pin
  skips h2/h3 promotion but can't constrain the handshake.)

### Changed

- The `http_version` hint's default meaning is unchanged (unset = auto-discovery), and the
  `http_version()` accessor still returns `Version`, reporting the unset default as `Http1_1`, so
  the public signature is unchanged. The only behavior change is for callers who explicitly set
  `Http1_1`/`Http2`/`Http3` and relied on the old "an explicit version still permits a different
  negotiated protocol" semantics: an explicit version now pins. An explicit `Http3` hint that fails
  its QUIC dial still falls back to auto-discovery.

- Concurrent first-time requests to an origin with no pooled connection now share a single connect
  instead of each racing to open its own. When the connection is multiplexed (HTTP/2 or HTTP/3) the
  whole burst shares it; this removes a multi-second first-request stall when several requests open a
  cold HTTP/3 origin at once. HTTP/1 cold-starts still open their own connections but no longer race
  redundantly.
- `Alt-Svc` advertisements are now recorded from HTTP/2 responses, not only HTTP/1.x and HTTP/3. On
  an HTTP/3-capable client, a recorded advertisement steers the next cold connection to that origin
  onto HTTP/3.
- On an HTTP/3-capable client, protocol selection now prefers reusing a live pooled connection over
  opening a new one. A pooled HTTP/2 connection is kept rather than migrated to HTTP/3, and a live
  HTTP/3 connection is reused even after its `Alt-Svc` advertisement expires (previously a lapsed
  advertisement could drop a still-open connection back to HTTP/2). A cold connection still prefers
  HTTP/3 when the origin advertises it. Explicit version pins (`Conn::with_http_version`) take strict
  precedence: a request pinned to HTTP/2 or HTTP/1 is never upgraded to HTTP/3.

## [0.9.5] - 2026-06-05

The theme of this release is protocol correctness / conformance, with a focus on http/1.x.

### Changed

- The trillium HTTP/1.x response parser (formerly behind the `parse` feature) is now the exclusive
  HTTP/1.x parser; the httparse-backed path has been removed and the `httparse` dependency
  dropped. The `parse` cargo feature is retained as a no-op for semver compatibility (it still
  forwards to the matching no-op feature in trillium-http).

### Security

- HTTP/1.x response smuggling: a malformed or duplicated `Content-Length` in a response was
  previously coerced — a non-digit value (including a leading `+`, which the standard library parses)
  or more than one `Content-Length` header silently fell back to read-to-close framing instead of
  being rejected. Because this client backs trillium-proxy, that framing disagreement with an
  upstream or downstream that trusts the literal `Content-Length` is a response-smuggling / desync
  vector. Such responses are now rejected, sharing the exact validation the server uses for request
  `Content-Length` so both halves of a proxy parse identically. The HTTP/3 client response path now
  applies the same `Content-Length` validation.
- `Transfer-Encoding` framing is now strict and consistent. The only transfer-coding trillium
  decodes is `chunked`, so a response's `Transfer-Encoding` must be exactly a single `chunked`;
  anything else — a coding list (`gzip, chunked`), a repeated `chunked` (which RFC 9112 §6.1 forbids
  a sender from producing), `chunked` not the final coding, or a value split across multiple header
  lines — is now rejected. Previously some of these were accepted and then mis-framed (decoded once,
  or silently downgraded to read-to-close), a framing/validation disagreement that is a
  response-smuggling vector. This matches the server's request-side `Transfer-Encoding` rule.

### Fixed

- A `Connection: close` split across multiple `Connection` header lines was ignored when deciding
  whether to reuse the connection, so the transport could be pooled and reused even though the peer
  asked to close it. All `Connection` lines and tokens are now scanned.
- A response status-line with a malformed status-code — more than three digits (`HTTP/1.1 2000 OK`)
  or otherwise not terminated by a space or end-of-line after three digits — is now rejected instead
  of being silently truncated to the first three digits.
- Regression: HTTP/1.1 responses are treated as keep-alive unless either side sends `Connection:
  close` (a 1.0 response still requires an explicit `Connection: keep-alive` on both
  sides). Previously reuse required an explicit `Connection: keep-alive` even on 1.1, so the
  overwhelming majority of 1.1 connections were closed and reopened instead of pooled.
- A connection abandoned before its response head arrived — a timeout or transport error
  mid-request — is no longer returned to the pool, where the next request could reuse the
  half-spent transport.
- The network response head now replaces, rather than merges with, any response headers a handler
  set synthetically before the round-trip (e.g. a `Content-Length` from `set_response_body`), so the
  parsed response can't end up with duplicated headers.

## [0.9.4] - 2026-06-03

### Added

- `Client::handler() -> &impl ClientHandler`

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
