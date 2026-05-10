# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.1](https://github.com/trillium-rs/trillium/compare/trillium-webtransport-v0.2.0...trillium-webtransport-v0.2.1) - 2026-05-10

### Other

- updated the following local packages: trillium-macros

## [0.2] - 2026-05-06

### Changed

- **Breaking:** `WebTransportConnection.upgrade: Upgrade` decomposed into concrete fields. The
  `upgrade()` and `upgrade_mut()` accessors are removed; replaced with `request_headers()` /
  `request_headers_mut()`, `response_headers()` / `response_headers_mut()`, `state()` /
  `state_mut()`, `path() -> Option<&str>`, `authority() -> Option<&str>`, and `peer_addr() ->
  SocketAddr`. The same struct is now constructed on both the server and client sides without
  needing to manufacture an `Upgrade`.
- `DEFAULT_MAX_DATAGRAM_BUFFER` is now `pub` at the crate root (was previously a private constant);
  useful for client code that wants to match the server's default datagram buffer cap.

### Added

- `WebTransportConnection` is now constructible from outside this crate, enabling the WebTransport
  client in `trillium-client` (cargo feature `webtransport`). The same type — and the same
  `accept_bidi` / `accept_uni` / `recv_datagram` / `open_bidi` / `open_uni` / `send_datagram` API —
  works whether you obtained it from a server-side handler or from
  `Client::webtransport(url).into_webtransport().await`.
- `Router` and `SessionRouter` are now `pub` (`#[doc(hidden)]`) so trillium-client can share the
  per-QUIC-connection multiplexing primitives. `Router::spawn_routing_task(self: Arc<Self>, quic,
  runtime)` is an idempotent helper that drains inbound WebTransport streams from the dispatcher and
  inbound datagrams from the QUIC connection, demuxing both to the right session. These are internal
  multiplexing primitives — most users will not interact with them directly.

## [0.1.0] - 2026-05-02

### Added
- Initial release: WebTransport handler for HTTP/3 sessions
