# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0](https://github.com/trillium-rs/trillium/compare/trillium-conn-id-v0.2.3...trillium-conn-id-v0.3.0) - 2026-04-08

### Added

- *(testing)* rename TestHandler to TestServer and misc testing improvements
- update all crates for new style of testing
- [**breaking**] use the extracted `type-set` crate instead of trillium_http::StateSet
- [**breaking**] eliminate async_trait

### Fixed

- fastrand seed change

### Other

- replace references to 0.3 with 1.0 in changelogs
- *(deps)* upgrade async-tungstenite
- Add readmes
- update all changelogs to reflect current status
- *(deps)* [**breaking**] update all deps
- edition 2024
- switch over to `///` from `/** */` comments
- add a rustfmt.toml and reformat
- release
- *(conn-id)* update seeded fastrand constants

### Changed
- Compatible with trillium 1.0

## [0.2.3](https://github.com/trillium-rs/trillium/compare/trillium-conn-id-v0.2.2...trillium-conn-id-v0.2.3) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 0.3

### Other
- release
- release
- Release only rustls
- release
- release

## [0.2.2](https://github.com/trillium-rs/trillium/compare/trillium-conn-id-v0.2.1...trillium-conn-id-v0.2.2) - 2024-01-02

### Other
- upgrade deps
- remove dependency carats
- conn-id patch: attempt to fix conn-id tests by introducing with_seed
- fastrand changed rng sequence from seed
- deps
- various nonbreaking dependency updates
- deps
- patch deps
- clippy is my copilot
- Update uuid requirement from 0.8.2 to 1.0.0
- update deps
