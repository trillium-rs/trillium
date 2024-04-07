# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.1](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.6.0...trillium-client-v0.6.1) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 0.3

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
