# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0](https://github.com/trillium-rs/trillium/compare/trillium-forwarding-v0.2.4...trillium-forwarding-v0.3.0) - 2026-04-04

### Added

- *(testing)* rename TestHandler to TestServer and misc testing improvements
- update all crates for new style of testing
- [**breaking**] add h3 support to client
- [**breaking**] remove Conn::inner and Conn::inner_mut
- [**breaking**] introduce ServerConfig
- [**breaking**] eliminate async_trait

### Other

- replace references to 0.3 with 1.0 in changelogs
- fix up broken docs links
- Add readmes
- update all changelogs to reflect current status
- some manual clippy fixes
- *(deps)* [**breaking**] update all deps
- edition 2024
- switch over to `///` from `/** */` comments
- further improvements to format settings
- add a rustfmt.toml and reformat
- release

### Changed
- Compatible with trillium 1.0

## [0.2.4](https://github.com/trillium-rs/trillium/compare/trillium-forwarding-v0.2.3...trillium-forwarding-v0.2.4) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Other
- release
- release
- clippy
- Release only rustls
- release
- release

## [0.2.3](https://github.com/trillium-rs/trillium/compare/trillium-forwarding-v0.2.2...trillium-forwarding-v0.2.3) - 2024-01-02

### Other
- updated the following local packages: trillium
