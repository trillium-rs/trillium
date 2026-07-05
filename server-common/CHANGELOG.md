# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.7.8] - 2026-07-04

### Fixed
- An HTTP/1.x connection accepted over a secure acceptor (direct TLS) now reports
  `Conn::is_secure() == true`. Previously only the HTTP/2 and HTTP/3 dispatch paths stamped the
  acceptor's secure-ness onto the conn, so an h1-over-TLS request read as insecure — affecting the
  `Secure` cookie attribute and URL-scheme derivation. h2/h3 were already correct.

## [0.7.7] - 2026-06-24

### Fixed

- `resolve_listener` now sets bound and `LISTEN_FDS`-inherited Unix listeners non-blocking, matching the TCP path. Previously a blocking Unix listener was handed to the runtime, which panicked the accept loop on tokio (recent versions reject registering a blocking file descriptor); smol was unaffected because async-io sets non-blocking at registration.

## [0.7.6] - 2026-06-18

The theme of this release is [RFC 9218 priority](https://datatracker.ietf.org/doc/rfc9218/) support.

### Added

- `QuicTransportSend::set_priority` — set a QUIC send stream's transmission priority relative to
  the connection's other streams; higher values are sent first when the connection is
  send-constrained. Defaulted to a no-op, so existing implementations are unaffected.
- HTTP/3 servers honor client-signaled RFC 9218 priority — the `priority` request header and
  `PRIORITY_UPDATE` frames — applying it to each request's QUIC send stream so more urgent
  responses are scheduled ahead of less urgent ones, and reprioritizing mid-response when a
  `PRIORITY_UPDATE` arrives.

## [0.7.5] - 2026-06-16

### Added

- `Connector::connect_to(Destination)` and the `Destination` type — describe a single connection
  attempt in full: whether it is `secure` (TLS), the host (for resolution and certificate
  validation), the port, any pre-resolved socket addresses to dial instead of resolving the host,
  and any per-connection ALPN. Build with `Destination::new_with_host` (resolve a domain, or dial
  pre-resolved addresses while validating against it) or `Destination::new_with_socket_addrs` (a
  bare-IP connection with no SNI); `Destination::from_url` performs the scheme/host/port extraction
  connectors previously did inline. Defaulted to reconstruct a url and call `connect`, so existing
  `Connector` implementations are unaffected. Taken by value so connectors can adjust it (e.g.
  clearing `secure` before delegating the TCP dial to an inner connector) without copying.
- `Destination::with_alpn` / `set_alpn` / `without_alpn` / `alpn` — advertise a specific ALPN
  protocol list for a single connection, overriding the connector's configured default. The
  override is an `Option`: `None` (the default, also `without_alpn`) leaves the connector's
  configured default in place, while `Some` advertises exactly the given list — including an empty
  list, which advertises no ALPN at all. TLS and QUIC connectors honor it; plaintext transports
  ignore it.
- `QuicEndpoint::connect_with_alpn` (and the matching `ArcedQuicEndpoint` method) — initiate a QUIC
  connection advertising a per-connection ALPN list, so one bound endpoint can negotiate different
  application protocols per connection (e.g. `h3` and `doq`) over the same UDP socket. Defaulted to
  ignore the ALPN and call `connect`, so existing `QuicEndpoint` implementations are unaffected.

## [0.7.4] - 2026-05-31

### Fixed

- 0.7.3 accidentally made ListenerConfig and IntoListenerAddr `#[doc(hidden)]`. This patch just
  removes that line

## [0.7.3] - 2026-05-31

### Added

- `QuicConfig::bind_with_socket(self, socket, runtime, info)` — new non-breaking trait method that
  takes a pre-claimed `std::net::UdpSocket` instead of a `SocketAddr`. Default implementation
  delegates back to `bind` via `socket.local_addr()`. Adapters should override to consume the
  pre-claimed socket directly.
- `ArcedQuicEndpoint::local_addr(&self) -> io::Result<SocketAddr>` — the local address the endpoint
  is bound to.
- `QuicEndpoint::local_addr(&self) -> io::Result<SocketAddr>` — the local address the endpoint is
  bound to. The default implementation returns `io::ErrorKind::Unsupported`; adapters with a bound
  UDP socket override it to return the actual address.
- `bind_reuse_port(addr) -> io::Result<TcpListener>` (Unix only, excluding Apple platforms) — bind a
  non-blocking std `TcpListener` with `SO_REUSEPORT` + `SO_REUSEADDR` for kernel connection fan-out
  across a listener group. Gated off on Apple platforms, where `SO_REUSEPORT` delivers every
  connection to a single listener rather than fanning out.
- `Config::listeners(self) -> ListenerConfig` — bridge from the single-listener `Config` into the
  multi-listener `ListenerConfig`, carrying over global server configuration (HTTP config, shared
  state, swansong, nodelay, max-connections, signals) but no listener binding (bind explicitly on
  the builder). Available before an acceptor/QUIC config is set.
- `BoundInfo::listeners() -> &[trillium::Listener]` — every listener the server is bound to. The
  builder produces one `trillium::Listener` per `bind_*` (TLS-vs-plaintext from each acceptor); the
  single-listener `Config` path synthesizes its own.
- Each `Conn` now carries its originating `trillium::Listener` in state (readable via
  `conn.state::<Listener>()`), across HTTP/1, HTTP/2, and HTTP/3. The ingress `SocketAddr` continues
  to be stamped alongside it for addressed listeners, so existing `conn.state::<SocketAddr>()`
  readers are unaffected; Unix-socket conns now have provenance they previously lacked.

## [0.7.2] - 2026-05-11

### Added

- `ServerHandle::block` for blocking on a spawned server from a sync context

## [0.7.1] - 2026-05-07

### Fixed

- HTTP/3: bidi streams now `RESET_STREAM` on stream-level protocol errors (RFC 9114 §4.1.2), and uni streams fire `CONNECTION_CLOSE` before the recv stream drops to avoid a `FINAL_SIZE_ERROR` race. Adopts `H3Connection::process_inbound_bidi_with_reset` / `process_inbound_uni_with_close` from `trillium-http` 1.2.

## [0.7.0] - 2026-05-05

### Changed

- Compatible with trillium 1.1

### Added

- `Binding::negotiated_alpn()` — forwards to the inner transport so runtime adapters can dispatch on ALPN regardless of which listener the connection came from
- HTTP/2 server support is now wired through the runtime adapter: when ALPN selects `h2`, connections are dispatched to the h2 driver instead of HTTP/1.1

## [0.6.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0
- Trillium 1.0 uses [Swansong](https://docs.rs/swansong) instead of Stopper; `config().with_stopper(stopper)` becomes `config().with_swansong(swansong)`
- `CloneCounter` and `CloneCounterObserver` removed; connection lifecycle tracking is now managed by Swansong guards
- `ServerHandle::stop()` → `ServerHandle::shut_down()` which returns `ShutdownCompletion` that can be awaited
- `ServerHandle::is_running()` removed
- `ObjectSafeConnector` is now private; use `ArcedConnector` instead (which adds `downcast_ref` / `downcast_mut` / `is` support)
- `Connector` trait: `spawn()` method removed; implement `type Runtime: RuntimeTrait`, `type Udp: UdpTransport`, and `fn runtime(&self) -> Self::Runtime` instead
- `Config` no longer implements `Clone`
- If you implement `Acceptor` or `Connector`, remove `#[async_trait]`

### Added
- `RuntimeTrait` — unified runtime abstraction with `spawn`, `block_on`, `delay`, `interval`, `timeout`, and signal hooks; implemented by `TokioRuntime`, `SmolRuntime`, `AsyncStdRuntime`
- `ArcedConnector` — type-erased connector wrapping an `Arc`, with downcast support
- `HttpContext` — Arc-shared per-server state (Swansong + TypeSet + HttpConfig) accessible from connections via `conn.shared_state()`
- `ServerHandle` is now `Clone`; awaiting it waits for server shutdown (`IntoFuture` impl)
- `ServerHandle::info().await` — async; waits for the server to finish binding and initialization, then returns a `BoundInfo`
- `BoundInfo` — immutable snapshot of server state after init: `tcp_socket_addr()`, `url()`, `unix_socket_addr()`, `state::<T>()`, `context()`
- `ServerHandle::runtime()` — retrieve the server's `Runtime`
- `Config::with_quic(q)` — attach a QUIC config for HTTP/3 support; takes any `impl QuicConfig`
- `QuicConfig`, `QuicEndpoint`, `QuicConnectionTrait`, `QuicConnection` — the three-layer abstraction for QUIC support (config → endpoint → connection); `()` impls disable H3 (the default)
- `UdpTransport` trait — async UDP socket abstraction for QUIC; implemented by `TokioUdpSocket`, `SmolUdpSocket`, `AsyncStdUdpSocket`
- `NoQuic` — uninhabited type implementing `QuicConnection` for the `()` disabled case

### Added
- *(server-common)* reexport all of ::url

## [0.5.1](https://github.com/trillium-rs/trillium/compare/trillium-server-common-v0.5.0...trillium-server-common-v0.5.1) - 2024-04-03

### Fixed
- *(server-common)* pass through poll_read_vectored in binding

## [0.5.0](https://github.com/trillium-rs/trillium/compare/trillium-server-common-v0.4.7...trillium-server-common-v0.5.0) - 2024-03-22

### Added
- propagate write_vectored calls in Binding
- *(server-common)* [**breaking**] put Config in an Arc instead of cloning

### Other
- clippy
- Release only rustls
- release

## [0.4.7](https://github.com/trillium-rs/trillium/compare/trillium-server-common-v0.4.6...trillium-server-common-v0.4.7) - 2024-01-02

### Other
- Update test-harness requirement from 0.1.1 to 0.2.0

## [0.4.6](https://github.com/trillium-rs/trillium/compare/trillium-server-common-v0.4.5...trillium-server-common-v0.4.6) - 2024-01-02

### Other
- update dependencies other than trillium-rustls
- use trillium-http@v0.3.8
- use trillium-http@v0.3.7
- deps
- bump trillium-http
- upgrade deps
- Fix documentation for `Connector::connect`
- remove dependency carats
