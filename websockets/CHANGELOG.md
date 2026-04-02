# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- Compatible with trillium 1.0
- `WebSocketConn::stopper()` → `WebSocketConn::swansong()` — trillium 1.0 uses [Swansong](https://docs.rs/swansong) instead of Stopper
- `pub use trillium_websockets::async_trait` removed; if you were importing `async_trait` through this crate, import it from the `async_trait` crate directly (or drop it entirely — `impl WebSocketHandler` no longer requires `#[async_trait]`)
- Updated to `async-tungstenite` 0.33

### Added
- `WebSocketConn::state_entry::<T>()` — entry API for connection state, mirrors `HashMap::entry`

## [0.6.6](https://github.com/trillium-rs/trillium/compare/trillium-websockets-v0.6.5...trillium-websockets-v0.6.6) - 2024-05-30

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
