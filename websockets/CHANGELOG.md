# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.9.2] - 2026-09-02

### Changed
- Replaced the deprecated `sha-1` crate with its maintained successor `sha1` 0.11. This drops
  `generic-array` from the dependency tree.

## [0.9.1] - 2026-08-27

### Fixed
- `WebSocketConn::querystring` returned an empty string for every connection, and has since
  0.8.0. Now it returns the querystring, as intended. Reported by
  [@dexgs](https://github.com/dexgs) in [#958](https://github.com/trillium-rs/trillium/pull/958).

## [0.9.0] - 2026-08-24

### Changed
- **Breaking**: handshakes are now checked against the request's origin. By default a handshake is
  accepted only if its `Origin` header names the same host as the request's `Host`/`:authority`,
  or if there is no `Origin` header, which means the client is not a browser. Browsers do not
  apply CORS to websockets, so without this check any page on the web could open an authenticated
  socket to your server using the visitor's cookies (cross-site websocket hijacking).

  An application whose pages and sockets are on different hosts must now name the page origin:
  `WebSocket::new(handler).allow_origins(["https://app.example.com"])`. Rejected handshakes get a
  `403 Forbidden` and a log line naming the refused origin and the method to call.

### Added
- `WebSocket::allow_origins`, `WebSocket::allow_origin_fn`, and `WebSocket::allow_any_origin`
  configure which pages may open a websocket.

## [0.8.3] - 2026-08-21

### Added
- `WebSocketConn::feed` enqueues a message into an internal write buffer without immediately
  writing it to the socket, allowing bursts of messages to coalesce into fewer socket writes.
  Buffered messages are written out when the buffer fills, when the conn (or the
  `WebSocketHandler` event loop) is polled for an inbound message and none is immediately
  available, or on `flush`/`send`.
- `WebSocketConn::flush` writes any buffered outbound messages to the socket.

### Changed
- Messages delivered via `WebSocketHandler`'s `OutboundStream` are now fed rather than
  individually flushed; the event loop flushes before waiting for new events, so outbound bursts
  coalesce. `WebSocketConn::send` is unchanged: it still flushes the message it sends.

## [0.8.2] - 2026-06-21

### Fixed
- A WebSocket handshake whose `Connection` header is split across multiple header lines (for example
  `Connection: keep-alive` and `Connection: Upgrade` on separate lines) is now recognized;
  previously the `Upgrade` token was missed and the handshake was not performed.

## [0.8.1] - 2026-06-10

### Fixed
- A WebSocket handshake carrying a `Sec-WebSocket-Version` other than `13` is now rejected with `426
  Upgrade Required`
- A WebSocket handshake is now only recognized for `GET` requests over HTTP/1.1.
- A WebSocket handshake over HTTP/1.0 is not accepted.

## [0.8.0] - 2026-05-05

### Added
- WebSockets over HTTP/2 (RFC 8441) — handled transparently when the request arrives via h2 with
  extended CONNECT. `WebSocketHandler::init` checks that the peer advertised
  `SETTINGS_ENABLE_CONNECT_PROTOCOL` and turns the handler into a no-op for that connection if not.

## [0.7.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0
- `WebSocketConn::stopper()` → `WebSocketConn::swansong()` — trillium 1.0 uses [Swansong](https://docs.rs/swansong) instead of Stopper
- `pub use trillium_websockets::async_trait` removed; if you were importing `async_trait` through this crate, import it from the `async_trait` crate directly (or drop it entirely — `impl WebSocketHandler` no longer requires `#[async_trait]`)
- Updated to `async-tungstenite` 0.33

### Added
- `WebSocketConn::state_entry::<T>()` — entry API for connection state, mirrors `HashMap::entry`

### Added
- deprecate set_state for insert_state

## [0.6.5](https://github.com/trillium-rs/trillium/compare/trillium-websockets-v0.6.4...trillium-websockets-v0.6.5) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Other
- release
- release
- clippy
- *(deps)* update base64 requirement from 0.21.5 to 0.22.0

## [0.6.4](https://github.com/trillium-rs/trillium/compare/trillium-websockets-v0.6.3...trillium-websockets-v0.6.4) - 2024-02-13

### Other
- *(deps)* update async-tungstenite requirement from 0.24.0 to 0.25.0
- *(deps)* update env_logger requirement from 0.10.1 to 0.11.0

## [0.6.3](https://github.com/trillium-rs/trillium/compare/trillium-websockets-v0.6.2...trillium-websockets-v0.6.3) - 2024-01-22

### Other
- Mark `WebSocketConn::new` as `doc(hidden)` since users shouldn't need it
- Add client WebSocket support

## [0.6.2](https://github.com/trillium-rs/trillium/compare/trillium-websockets-v0.6.1...trillium-websockets-v0.6.2) - 2024-01-02

### Other
- updated the following local packages: trillium-http

## [0.6.1](https://github.com/trillium-rs/trillium/compare/trillium-websockets-v0.6.0...trillium-websockets-v0.6.1) - 2024-01-02

### Other
- update dependencies other than trillium-rustls
