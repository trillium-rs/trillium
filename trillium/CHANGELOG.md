# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.18](https://github.com/trillium-rs/trillium/compare/trillium-v0.2.17...trillium-v0.2.18) - 2024-04-04

### Fixed
- *(trillium)* move futures-lite to dev-dependencies

## [0.2.17](https://github.com/trillium-rs/trillium/compare/trillium-v0.2.16...trillium-v0.2.17) - 2024-03-22

### Added
- *(trillium)* improve log message when calling `Arc<Handler>::init` on a clone

### Other
- clippy

## [0.2.16](https://github.com/trillium-rs/trillium/compare/trillium-v0.2.15...trillium-v0.2.16) - 2024-02-09

### Fixed
- *(trillium)* downgrade Arc<Handler>::init from a panic to a warning
- *(testing)* TestTransport behaves like TcpStream regarding closure

### Other
- *(trillium)* add test for is_disconnected

## [0.2.15](https://github.com/trillium-rs/trillium/compare/trillium-v0.2.14...trillium-v0.2.15) - 2024-02-05

### Fixed
- *(trillium)* fix trillium-http dependency

## [0.2.14](https://github.com/trillium-rs/trillium/compare/trillium-v0.2.13...trillium-v0.2.14) - 2024-02-05

### Added
- *(trillium)* reexpose trillium-http features
- *(http)* cancel on disconnect

### Other
- *(trillium)* add liveness (cancel-on-disconnect) test

## [0.2.13](https://github.com/trillium-rs/trillium/compare/trillium-v0.2.12...trillium-v0.2.13) - 2024-01-02

### Other
- updated the following local packages: trillium-http

## [0.2.12](https://github.com/trillium-rs/trillium/compare/trillium-v0.2.11...trillium-v0.2.12) - 2024-01-02

### Other
- update dependencies other than trillium-rustls
- use trillium-http@v0.3.8
- use trillium-http@v0.3.7
- deps
- 📎💬
- bump trillium-http
- upgrade deps
- Expose `start_time()` on `trillium::Conn` to avoid needing `inner()`
