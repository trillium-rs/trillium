# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0](https://github.com/trillium-rs/trillium/compare/trillium-server-common-v0.5.2...trillium-server-common-v0.6.0) - 2026-04-04

### Added

- [**breaking**] rename http_config to config
- [**breaking**] rename ServerConfig to HttpContext
- *(trillium)* introduce façade types for Upgrade and RequestBody
- [**breaking**] add h3 support to client
- [**breaking**] webtransport and some tidying of h3/quic
- auto-populate alt-svc response header
- [**breaking**] hypertext transfer protocol, three
- use `From<Arc<impl RuntimeTrait>>` to create a `Runtime`
- [**breaking**] introduce ServerConfig
- [**breaking**] introduce Runtime
- *(server-common)* [**breaking**] remove Clone for Config
- [**breaking**] use swansong instead of stopper + clone counter
- *(client)* [**breaking**] add support for client timeouts
- [**breaking**] eliminate async_trait
- [**breaking**] remove `impl Handler for Arc<impl Handler>`
- *(server-common)* [**breaking**] put Info in an Arc for now

### Fixed

- client now is appropriately factored, uses H3Connection

### Other

- replace references to 0.3 with 1.0 in changelogs
- silence the log error when rustls emits an unexpected eof
- fix up broken docs links
- Add readmes
- update all changelogs to reflect current status
- some manual clippy fixes
- clippy auto fix
- *(deps)* [**breaking**] update all deps
- documentation pass
- edition 2024
- switch over to `///` from `/** */` comments
- further improvements to format settings
- add a rustfmt.toml and reformat
- release
- release
- release

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
- `QuicConfig`, `QuicBinding`, `QuicConnection` traits — the three-layer abstraction for QUIC support (config → binding → connection); `()` impls disable H3 (the default)
- `UdpTransport` trait — async UDP socket abstraction for QUIC; implemented by `TokioUdpSocket`, `SmolUdpSocket`, `AsyncStdUdpSocket`
- `NoQuic` — uninhabited type implementing `QuicConnection` for the `()` disabled case

## [0.5.2](https://github.com/trillium-rs/trillium/compare/trillium-server-common-v0.5.1...trillium-server-common-v0.5.2) - 2024-04-04

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
