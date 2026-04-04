# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0](https://github.com/trillium-rs/trillium/compare/trillium-tokio-v0.4.0...trillium-tokio-v0.5.0) - 2026-04-04

### Added

- [**breaking**] add h3 support to client
- [**breaking**] hypertext transfer protocol, three
- use `From<Arc<impl RuntimeTrait>>` to create a `Runtime`
- [**breaking**] introduce ServerConfig
- [**breaking**] introduce Runtime
- [**breaking**] use swansong instead of stopper + clone counter
- *(client)* [**breaking**] add support for client timeouts
- [**breaking**] eliminate async_trait

### Fixed

- client now is appropriately factored, uses H3Connection
- don't even compile unreachable types

### Other

- replace references to 0.3 with 1.0 in changelogs
- *(deps)* upgrade async-tungstenite
- Add readmes
- update all changelogs to reflect current status
- clippy auto fix
- *(deps)* [**breaking**] update all deps
- remove panic demonstration from tokio example
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
- Free functions `spawn` and `block_on` removed; use `TokioRuntime::default().spawn(fut)` and `TokioRuntime::default().block_on(fut)` respectively
- `ClientConfig::spawn(fut)` → `ClientConfig::runtime().spawn(fut)`

### Added
- `TokioRuntime` implementing `RuntimeTrait`, with methods `spawn`, `block_on`, `delay`, `interval`, `timeout`, and signal hooks
- `TokioUdpSocket` — UDP transport type implementing `UdpTransport`, used by [`trillium-quinn`](https://docs.rs/trillium-quinn) for HTTP/3
- `Config::spawn(handler)` now returns a `ServerHandle` that is `Clone` and covers the full server lifecycle: `await` it to wait for shutdown, call `handle.info().await` to wait for the server to finish binding and get a `BoundInfo` (bound address, URL, shared state), and `handle.shut_down()` to initiate graceful shutdown
- HTTP/3 support: `config().with_quic(trillium_quinn::QuicConfig::from_single_cert(&cert_pem, &key_pem))` — see the [trillium changelog](https://docs.rs/trillium) for details

## [0.4.0](https://github.com/trillium-rs/trillium/compare/trillium-tokio-v0.3.4...trillium-tokio-v0.4.0) - 2024-04-04

### Added
- *(tokio)* [**breaking**] use trillium-server-common 0.5

### Other
- release
- release
- release
- clippy
- *(deps)* update env_logger requirement from 0.10.1 to 0.11.0

## [0.3.4](https://github.com/trillium-rs/trillium/compare/trillium-tokio-v0.3.3...trillium-tokio-v0.3.4) - 2024-01-02

### Other
- updated the following local packages: trillium-http, trillium-server-common

## [0.3.3](https://github.com/trillium-rs/trillium/compare/trillium-tokio-v0.3.2...trillium-tokio-v0.3.3) - 2024-01-02

### Other
- client runtime adapters: perform async dns lookups
- use trillium-http@v0.3.8
- use trillium-http@v0.3.7
- deps
- bump trillium-http
- upgrade deps
- remove dependency carats
- continue to struggle with port collisions in ci
- allow nodelay to be set on prebound servers
