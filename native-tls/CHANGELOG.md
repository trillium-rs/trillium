# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).


## [0.6.2] - 2026-05-06

### Fixed

- `NativeTlsAcceptor::from_cert_and_key` now tries [`Identity::from_pkcs8`]
  first and falls back to packaging the cert chain and key into an in-memory
  PKCS#12 archive (via [`Identity::from_pkcs12`]) only when that path fails.
  This works around macOS Secure Transport's `errSecUnknownFormat` failure on
  EC keys (e.g. ACME-issued tailnet/Let's Encrypt certs) while keeping the
  fast PKCS#8 path on Linux and Windows.

## [0.6.1] - 2026-05-06

### Added

- `NativeTlsAcceptor::from_cert_and_key(cert, key)` — recommended primary constructor matching the
  input signature used by `trillium-rustls` and `trillium-openssl`. Accepts PEM cert chains and PEM
  keys in PKCS#8, PKCS#1 (RSA), or SEC1 (EC) form, normalizing to PKCS#8 before handing off to
  `native_tls::Identity`. Either argument may be a concatenated bundle containing both cert and key.

### Changed

- README and example now lead with `from_cert_and_key`. `from_pkcs12` is
  retained for callers with password-protected archives, and `from_pkcs8` is
  retained for backwards compatibility.

## [0.6.0] - 2026-05-06

### Changed

- Compatible with trillium 1.1

## [0.5.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0
- Trillium 1.0 uses [Swansong](https://docs.rs/swansong) instead of Stopper; `config().with_stopper(stopper)` becomes `config().with_swansong(swansong)`
- `NativeTlsConfig::spawn(fut)` → `NativeTlsConfig::runtime().spawn(fut)`

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
