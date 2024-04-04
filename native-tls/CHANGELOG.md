# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0](https://github.com/trillium-rs/trillium/compare/trillium-native-tls-v0.3.3...trillium-native-tls-v0.4.0) - 2024-04-04

### Added
- *(native-tls)* [**breaking**] split NativeTlsTransport into client and server

### Other
- release

## [0.3.3](https://github.com/trillium-rs/trillium/compare/trillium-native-tls-v0.3.2...trillium-native-tls-v0.3.3) - 2024-04-03

### Fixed
- *(native-tls)* pass through poll_{write,read}_vectored

### Other
- release
- *(deps)* update env_logger requirement from 0.10.1 to 0.11.0

## [0.3.2](https://github.com/trillium-rs/trillium/compare/trillium-native-tls-v0.3.1...trillium-native-tls-v0.3.2) - 2024-01-02

### Other
- updated the following local packages: trillium-server-common

## [0.3.1](https://github.com/trillium-rs/trillium/compare/trillium-native-tls-v0.3.0...trillium-native-tls-v0.3.1) - 2024-01-02

### Fixed
- fix tls deps

### Other
- Update identity.p12
- upgrade deps
- remove dependency carats
- fmt
- run mkcert in ubuntu only, use it optionally in tests
- add NativeTlsAcceptor::from_pkcs8
- add tests
- native-tls bugfix: use correct url when connecting
- (cargo-release) version 0.3.0
- (cargo-release) version 0.2.0
- Make Transport an actual trait
- Update async-native-tls requirement from 0.4.0 to 0.5.0
- clippy fixes
- patch deps
- Update env_logger requirement from 0.9.0 to 0.10.0
- Update async-native-tls requirement from 0.3.3 to 0.4.0
- 2021
- version bumps
- upgrade env_logger
- remove a dbg and add a clippy to keep me from doing that again
- lint changed from missing_crate_level_docs to rustdoc::missing_crate_level_docs
- update dependencies
- 🖇 that clippy is always right 📎
- missed a version
- tidy cargo.tomls
- deny missing docs everywhere
- document and tidy client
- more docs, DevLogger → Logger::new()
- client connector implementations
- address all non-missing-docs lints
- bump all deps (wip)
- propagate conn method renaming and fix tests
- break the build by requiring all of the docs that are currently missing
- cargo fixed
- 🎶 say my name, say my name 🎵
- udeps
- simplify server run, using 12-factor style PORT and HOST by default
- include placeholder certs
- make selection of tls implementation independent from runtime
