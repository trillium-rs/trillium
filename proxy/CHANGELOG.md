# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0](https://github.com/trillium-rs/trillium/compare/trillium-proxy-v0.5.6...trillium-proxy-v0.6.0) - 2026-04-01

### Added

- *(client)* [**breaking**] with_default_pool is now actually the default
- further improvements on client and proxy for h3
- [**breaking**] add h3 support to client
- [**breaking**] hypertext transfer protocol, three
- [**breaking**] remove Conn::inner and Conn::inner_mut
- [**breaking**] use swansong instead of stopper + clone counter
- [**breaking**] eliminate async_trait

### Fixed

- client now is appropriately factored, uses H3Connection

### Other

- Add readmes
- update all changelogs to reflect current status
- *(deps)* [**breaking**] update all deps
- edition 2024
- switch over to `///` from `/** */` comments
- further improvements to format settings
- add a rustfmt.toml and reformat

### Changed
- Compatible with trillium 0.3

## [0.5.6](https://github.com/trillium-rs/trillium/compare/trillium-proxy-v0.5.5...trillium-proxy-v0.5.6) - 2026-02-27

### Fixed

- *(proxy)* Inappropriately stripped Connection / Upgrade in SwitchingProtocol

## [0.5.5](https://github.com/trillium-rs/trillium/compare/trillium-proxy-v0.5.4...trillium-proxy-v0.5.5) - 2024-05-30

### Added
- deprecate set_state for insert_state

## [0.5.4](https://github.com/trillium-rs/trillium/compare/trillium-proxy-v0.5.3...trillium-proxy-v0.5.4) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 0.3

### Other
- release
- release
- release
- release

## [0.5.3](https://github.com/trillium-rs/trillium/compare/trillium-proxy-v0.5.2...trillium-proxy-v0.5.3) - 2024-03-22

### Fixed
- *(proxy)* use Connector and ObjectSafeConnector from trillium_client

### Other
- clippy
- *(clippy)* fix two clippies
- *(deps)* update env_logger requirement from 0.10.1 to 0.11.0
- release

## [0.5.2](https://github.com/trillium-rs/trillium/compare/trillium-proxy-v0.5.1...trillium-proxy-v0.5.2) - 2024-01-02

### Other
- updated the following local packages: trillium-http, trillium-server-common, trillium-client

## [0.5.1](https://github.com/trillium-rs/trillium/compare/trillium-proxy-v0.5.0...trillium-proxy-v0.5.1) - 2024-01-02

### Other
- whatever clippy wants, clippy gets
- use pooling in example
- proxy patch: only stream the request body if it's present, and handle get requests with bodies
- proxy patch bugfix: forward proxy connect needs to halt
