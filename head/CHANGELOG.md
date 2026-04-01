# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0](https://github.com/trillium-rs/trillium/compare/trillium-head-v0.2.3...trillium-head-v0.3.0) - 2026-04-01

### Added

- *(testing)* rename TestHandler to TestServer and misc testing improvements
- update all crates for new style of testing
- [**breaking**] add h3 support to client
- [**breaking**] remove Conn::inner and Conn::inner_mut
- [**breaking**] eliminate async_trait

### Fixed

- trillium-head missing a ;

### Other

- Add readmes
- update all changelogs to reflect current status
- edition 2024
- switch over to `///` from `/** */` comments
- add a rustfmt.toml and reformat

### Changed
- Compatible with trillium 0.3

## [0.2.3](https://github.com/trillium-rs/trillium/compare/trillium-head-v0.2.2...trillium-head-v0.2.3) - 2024-05-30

### Added
- deprecate set_state for insert_state

## [0.2.2](https://github.com/trillium-rs/trillium/compare/trillium-head-v0.2.1...trillium-head-v0.2.2) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 0.3

### Other
- release
- release
- clippy
- Release only rustls
- release
- release

## [0.2.1](https://github.com/trillium-rs/trillium/compare/trillium-head-v0.2.0...trillium-head-v0.2.1) - 2024-01-02

### Other
- remove dependency carats
- 2021
- add caching header support
- add examples and docs for several new crates
