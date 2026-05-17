# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Reading an HTTP/1.1 chunked-encoded body — request bodies in the server role, response bodies in the client role — could in rare cases fail to decode despite the wire being well-formed, surfacing as one of `chunk header too long`, `invalid chunk size`, `ConnectionAborted`, or `UnexpectedEof`. The triggers all sat at the intersection of partial chunk-size headers (caused by transport segmentation landing inside the few-byte chunk-size header window) and content already buffered for processing (either residual from the conn's pre-read scratch, or partial header bytes stashed by a prior poll). Well-behaved clients use sensible chunk sizes, and reverse proxies typically re-frame chunked bodies before forwarding to the backend, so traffic in typical deployments was very unlikely to hit any of these. Decode errors are now surfaced only for genuinely malformed bodies or actual transport closure. Outbound chunked encoding (the write path in either role) was never affected.

## [1.2.2] - 2026-05-15

### Added

- `Error::Other` and the `Error::other(impl Error)` constructor — a catchall variant for application-level errors that need to flow through `trillium_http::Result`.
- `KnownHeaderName::CdnCacheControl`

### Changed

- `From<ReceivedBody<'static, _>> for Body` now propagates trailers from
  the source body through the conversion. Behavior change for the
  trailer-carrying case; equivalent for bodies without trailers.

## [1.2.1] - 2026-05-11

### Fixed

- HTTP/2 client: interim 1xx HEADERS frames (early hints, etc.) are now discarded instead of being latched as the final response. Per RFC 9113 §8.1 a response may include zero or more informational HEADERS before the final, and per RFC 9110 §15.2 / RFC 8297 §2 their headers must not be merged into the final. `101 Switching Protocols` continues to be treated as a final response. An interim HEADERS frame that erroneously carries `END_STREAM` now surfaces `ConnectionAborted` to the conn task instead of hanging.

## [1.2.0] - 2026-05-07

### Added

- `H3Connection::process_inbound_bidi_with_reset` — process a bidi request stream with a caller-supplied closure that issues `RESET_STREAM` on stream-level protocol errors, as required by RFC 9114 §4.1.2.
- `H3Connection::process_inbound_uni_with_close` — process a uni stream with a caller-supplied closure that fires `CONNECTION_CLOSE` while the recv stream is still alive, avoiding a `FINAL_SIZE_ERROR` race with the peer's response to STOP_SENDING.

### Deprecated

- `H3Connection::process_inbound_bidi` — use `process_inbound_bidi_with_reset` instead.
- `H3Connection::process_inbound_uni` — use `process_inbound_uni_with_close` instead.

### Fixed

[h3spec](https://github.com/kazu-yamamoto/h3spec) identified the following minor violations in trillium's h3 implementation, primarily focused on error handling. All of these are fixed in 1.2.0:

- RFC 9114 §4.1.2 — stream-level errors (notably `H3_MESSAGE_ERROR`) MUST RST the bidi stream.
- RFC 9114 §4.1.1 / §4.2 / §4.3.1 — malformed messages (duplicated pseudos, unknown pseudos, uppercase header bytes) are `H3_MESSAGE_ERROR`.
- RFC 9114 §4.3.1 — schemes with mandatory authority component (http/https) require `:authority` or `Host` on non-`CONNECT` requests.
- RFC 9114 §6.2.1 — first frame on the peer's control stream must be `SETTINGS`. Non-`SETTINGS` first frame OR a malformed first frame →
  `H3_MISSING_SETTINGS`.
- RFC 9114 §6.2.1 + RFC 9204 §4.2 — closure of control or QPACK streams is `H3_CLOSED_CRITICAL_STREAM`.
- RFC 9114 §7.2.1 / §7.2.2 / §7.2.4 / §7.2.5 — control stream must reject `DATA`, `HEADERS`, `PUSH_PROMISE`, second `SETTINGS`, and the WebTransport
  `0x41` signal as `H3_FRAME_UNEXPECTED`.
- RFC 9114 §4.1 — first-frame decode failure on a request bidi stream is `H3_FRAME_UNEXPECTED`.
- RFC 9204 §3.1 — invalid static-table index in a field-line representation is `QPACK_DECOMPRESSION_FAILED`.
- RFC 9204 §4.1.3 — Set Dynamic Table Capacity exceeding the limit is `QPACK_ENCODER_STREAM_ERROR`.
- RFC 9204 §4.4.3 — Insert Count Increment of 0 is `QPACK_DECODER_STREAM_ERROR`.
- RFC 9204 §6 — QPACK errors are connection-level, not stream-level.
- RFC 9114 §8.1 / RFC 9204 §6 — correct close error code on the wire.

## [1.1.0] - 2026-05-05

This release adds http/2 support.

### Added
- `pub mod h2` — HTTP/2 protocol primitives: HPACK encode/decode, h2 framing, `H2Connection`, `H2Driver`, `H2Transport`. HTTP/2 is automatically negotiated when ALPN selects `h2` or via prior-knowledge cleartext ("h2c"). 146/146 [h2spec](https://github.com/summerwind/h2spec) cases pass.
- HTTP/2 extended CONNECT (RFC 8441) — opt in via `HttpConfig::with_extended_connect_enabled()`; required for WebSockets-over-h2.
- `KnownHeaderName::Refresh`
- `Conn::h2_connection()`, `Conn::h2_stream_id()`, `Conn::h3_stream_id()` — for handlers that want to interact with the underlying h2/h3 stream
- `Upgrade::h2_connection`, `Upgrade::h2_stream_id`, `Upgrade::h3_stream_id` (and `_mut` / `set_` / `with_` / `take_` variants where applicable) — used by `trillium-websockets` for WS-over-h2 (RFC 8441)
- Various `HttpConfig::h2_*` tuning knobs: `h2_initial_connection_window_size`, `h2_initial_stream_window_size`, `h2_max_stream_recv_window_size`, `h2_max_concurrent_streams`, `h2_max_frame_size`
- `HttpConfig::dynamic_table_capacity` (and setter / `with_` / `_mut` variants) — HPACK/QPACK encoder dynamic table capacity, shared between h2 and h3
- `HttpConfig::recent_pairs_size` (and setter / `with_` / `_mut` variants) — per-connection ring size for the HPACK/QPACK encoder's recent-pairs predictor
- `HttpConfig::h3_blocked_streams` (and setter / `with_` / `_mut` variants) — maximum number of HTTP/3 streams that may be blocked waiting for QPACK dynamic-table updates
- `Upgrade::response_headers: Headers` — the response headers that had been set on the underlying `Conn` before the upgrade was negotiated. These have already been sent to the peer; preserved here so post-upgrade code can inspect what was sent. `response_headers_mut`, `set_response_headers`, `with_response_headers`, and `into_response_headers` round out the accessor surface.
- `Upgrade::status` (with `_mut` / `set_` / `take_` / `with_` variants) — `Option<Status>` carrying the response status sent before the upgrade
- `Upgrade::start_time` (with `_mut` / `set_` / `with_` variants) — `Instant` recording when the Conn that became this Upgrade was constructed
- `H3Connection::peer_settings_ready() -> PeerSettingsReady<'_>` (gated on the `unstable` feature) — async future that resolves to `Some(H3Settings)` once the inbound control stream has applied the peer's SETTINGS frame, or `None` if the connection shut down before SETTINGS arrived. Required for senders of extended-CONNECT requests (RFC 9220 §3 — the spec forbids sending a `:protocol` HEADERS until the peer has advertised `SETTINGS_ENABLE_CONNECT_PROTOCOL`). On a pooled connection that has already exchanged SETTINGS, the future resolves on the first poll. Multiple awaiters on the same connection are supported.

The existing sync `H3Connection::peer_settings(&self) -> Option<&H3Settings>` accessor is unchanged.

## [1.0.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0
- `StateSet` renamed to `TypeSet` and extracted to the [`type-set`](https://docs.rs/type-set) crate; re-exported as `trillium_http::TypeSet`
- Trillium 1.0 uses [Swansong](https://docs.rs/swansong) instead of Stopper; `Conn::stopper()` → `Conn::swansong()`
- `Error` variants renamed for consistency: `MalformedHeader` → `InvalidHeaderValue`, `PartialHead` → `InvalidHead`, `MissingVersion` → `InvalidVersion`, `UnrecognizedStatusCode`/`MissingStatusCode` → `InvalidStatus`/`MissingStatus`; `HeaderMissing` and `UnexpectedHeader` now carry `HeaderName<'static>` instead of `&'static str`; `UnsupportedVersion` now carries `Version` instead of `u8`
- `Version::Http2_0` renamed to `Version::Http2`; `Version::Http3_0` renamed to `Version::Http3`
- `Upgrade` is now `#[non_exhaustive]`; `Upgrade::buffer` changed from `Option<Vec<u8>>` to `Buffer`; `Upgrade::stopper` renamed to `Upgrade::swansong`; `Upgrade::peer_ip: Option<IpAddr>` added
- `ReceivedBody` no longer implements `Stream`; use `AsyncRead` instead
- `Headers::contains_ignore_ascii_case` removed (was deprecated)
- `Headers::append` and `Headers::try_insert_with` now return `&mut HeaderValues` instead of `()`
- `set_*` setters on `Conn` (e.g. `set_status`, `set_host`) now return `&mut Self`, enabling chaining
- Handler futures in `Conn::map` and friends no longer require `Send`
- `pub mod transport` removed — the `Transport` trait is now at `trillium::Transport`
- `Body::new_streaming` no longer requires a `Sync` reader.
- `Conn::request_body` is synchronous now. 100-continue is sent, if necessary, on first read from the body.
- `ReceivedBody` no longer implements `IntoFuture` to make the transition to request_body being synchronous easier.

### Added
- `Headers::entry()` — Entry API for inserting/modifying headers, mirroring `HashMap::entry`
- `parse` feature — opt-in alternative header parser (bypasses httparse; groundwork for H3)
- `HttpContext` is now public — Arc-shared per-server state (Swansong + TypeSet + HttpConfig) passed to every connection
- `pub mod h3` — HTTP/3 protocol primitives: QPACK encode/decode, H3 framing, `H3Connection`, `H3Body`, `H3Error`; used by [`trillium-quinn`](https://docs.rs/trillium-quinn) and other QUIC adapter crates

## [0.3.16](https://github.com/trillium-rs/trillium/compare/trillium-http-v0.3.15...trillium-http-v0.3.16) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 0.3

### Fixed
- remove unreleased Upgrade::request_headers and Upgrade::request_headers_mut
- *(trillium)* fix the flaky liveness test

## [0.3.15](https://github.com/trillium-rs/trillium/compare/trillium-http-v0.3.14...trillium-http-v0.3.15) - 2024-03-22

### Added
- *(http)* sort Host and Date headers first
- *(test)* add corpus tests

### Other
- clippy
- *(http)* document addition of is_valid

## [0.3.14](https://github.com/trillium-rs/trillium/compare/trillium-http-v0.3.13...trillium-http-v0.3.14) - 2024-02-08

### Added
- *(http)* add the notion of closure to synthetic bodies

### Fixed
- *(http)* fix Conn::is_disconnected logic
- *(http)* fix synthetic body AsyncRead implementation for large bodies

## [0.3.13](https://github.com/trillium-rs/trillium/compare/trillium-http-v0.3.12...trillium-http-v0.3.13) - 2024-02-05

### Added
- *(http)* fix http-compat cargo feature specification
- *(http)* relax handler constraint to be FnMut instead of Fn
- *(http)* cancel on disconnect

### Other
- *(http)* appease the clipmaster

## [0.3.12](https://github.com/trillium-rs/trillium/compare/trillium-http-v0.3.11...trillium-http-v0.3.12) - 2024-01-24

### Fixed
- *(security)* allow all tchar in header names
- *(security)* handling of unsafe characters in outbound header names and values

### Other
- *(deps)* update env_logger requirement from 0.10.1 to 0.11.0

## [0.3.11](https://github.com/trillium-rs/trillium/compare/trillium-http-v0.3.10...trillium-http-v0.3.11) - 2024-01-02

### Other
- use #[test(harness)] instead of #[test(harness = harness)]
- Update test-harness requirement from 0.1.1 to 0.2.0

## [0.3.10](https://github.com/trillium-rs/trillium/compare/trillium-http-v0.3.9...trillium-http-v0.3.10) - 2024-01-02

### Other
- Update smol requirement from 1.3.0 to 2.0.0
- update dependencies other than trillium-rustls
- http patch reversion: set Server header before request again
- http patch feature: serde for version
- http patch: don't send explicit connection: keep-alive
