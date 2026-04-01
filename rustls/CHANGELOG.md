# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.10.0](https://github.com/trillium-rs/trillium/compare/trillium-rustls-v0.9.0...trillium-rustls-v0.10.0) - 2026-04-01

### Added

- further improvements on client and proxy for h3
- [**breaking**] add h3 support to client
- [**breaking**] introduce ServerConfig
- [**breaking**] introduce Runtime
- [**breaking**] use swansong instead of stopper + clone counter
- *(client)* [**breaking**] add support for client timeouts
- [**breaking**] eliminate async_trait

### Fixed

- client now is appropriately factored, uses H3Connection

### Other

- fix up broken docs links
- remove some straggling dbg! macros
- Add readmes
- update all changelogs to reflect current status
- clippy auto fix
- *(deps)* [**breaking**] update all deps
- edition 2024
- switch over to `///` from `/** */` comments
- further improvements to format settings
- add a rustfmt.toml and reformat

### Changed
- Compatible with trillium 0.3
- Trillium 0.3 uses [Swansong](https://docs.rs/swansong) instead of Stopper; `config().with_stopper(stopper)` becomes `config().with_swansong(swansong)`
- `RustlsConfig::spawn(fut)` → `RustlsConfig::runtime().spawn(fut)`

## [0.8.1](https://github.com/trillium-rs/trillium/compare/trillium-rustls-v0.8.0...trillium-rustls-v0.8.1) - 2024-06-29

### Fixed
- Require rustls-platform-verifier 0.3.2 to avoid multiple crypto backends

## [0.8.0](https://github.com/trillium-rs/trillium/compare/trillium-rustls-v0.7.0...trillium-rustls-v0.8.0) - 2024-04-12

### Added
- *(rustls)* [**breaking**] change how crypto providers are selected

See crate-level documentation for more on how the new features work. This is only a breaking change if you were using `default-features = false` AND not enabling either `ring` or `aws-lc-rs`. In that case you'll need to enable `custom-crypto-provider` on this crate, which brings in no additional dependencies but makes the possibility of a runtime panic due to crypto feature selection opt-in. Without this feature, misconfiguration (`default-features = false` without a crypto provider`) will be a compile-time error. 

### Other
- release

## [0.7.0](https://github.com/trillium-rs/trillium/compare/trillium-rustls-v0.6.0...trillium-rustls-v0.7.0) - 2024-04-03

### Added
- [**breaking**] upgrade rustls, use platform verifier

### Other
- *(actions)* tell cargo-udeps to ignore webpki-roots
- release

## [0.6.0](https://github.com/trillium-rs/trillium/compare/trillium-rustls-v0.5.0...trillium-rustls-v0.6.0) - 2024-01-24

### Other
- *(deps)* update env_logger requirement from 0.10.1 to 0.11.0
- Support using aws-lc-rs instead of ring
- Rename trillium-rustls `client` example so it doesn't conflict

## [0.5.0](https://github.com/trillium-rs/trillium/compare/trillium-rustls-v0.4.2...trillium-rustls-v0.5.0) - 2024-01-04

### Added
- *(rustls)* [**breaking**] add client and server features
- *(rustls)* [**breaking**] update trillium-rustls, switching to futures-rustls

## [0.4.2](https://github.com/trillium-rs/trillium/compare/trillium-rustls-v0.4.1...trillium-rustls-v0.4.2) - 2024-01-02

### Other
- updated the following local packages: trillium-server-common

## [0.4.1](https://github.com/trillium-rs/trillium/compare/trillium-rustls-v0.4.0...trillium-rustls-v0.4.1) - 2024-01-02

### Other
- upgrade deps
- remove dependency carats
- Make native root support optional
- deps
- fmt
