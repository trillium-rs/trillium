# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.7](https://github.com/trillium-rs/trillium/compare/trillium-macros-v0.0.6...trillium-macros-v0.0.7) - 2026-04-04

### Added

- *(testing)* rename TestHandler to TestServer and misc testing improvements
- update all crates for new style of testing
- [**breaking**] add h3 support to client
- [**breaking**] introduce ServerConfig
- [**breaking**] eliminate async_trait

### Other

- replace references to 0.3 with 1.0 in changelogs
- update all changelogs to reflect current status
- some manual clippy fixes
- clippy auto fix
- *(deps)* [**breaking**] update all deps
- edition 2024
- further improvements to format settings
- add a rustfmt.toml and reformat

### Changed
- Compatible with trillium 1.0

## [0.0.6](https://github.com/trillium-rs/trillium/compare/trillium-macros-v0.0.5...trillium-macros-v0.0.6) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Other
- clippy

## [0.0.5](https://github.com/trillium-rs/trillium/compare/trillium-macros-v0.0.4...trillium-macros-v0.0.5) - 2024-01-02

### Other
- update dependencies other than trillium-rustls
- Add test for `derive(Transport)`
- Add (and document) macro for `derive(Transport)`
- Tweak error message to reference "method" rather than "function"
- Make error message mention trait-specific alternative annotations
- Fix typo: s/erro/error/
- README.md: Document derives for `AsyncRead` and `AsyncWrite`
- README.md: Mention derives for `AsyncRead` and `AsyncWrite` too
- README.md: Wrap `#[handler]` and `FN_NAME` in code blocks
- deps
- upgrade deps
- Update futures-lite requirement from 1.13.0 to 2.0.0
- deps
- fmt
