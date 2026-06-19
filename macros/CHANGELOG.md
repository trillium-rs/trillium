# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0](https://github.com/trillium-rs/trillium/compare/trillium-macros-v0.1.0...trillium-macros-v0.2.0) - 2026-06-19

### Added

- *(http)* http/2 support
- normalization pass
- *(testing)* rename TestHandler to TestServer and misc testing improvements
- update all crates for new style of testing
- [**breaking**] add h3 support to client
- [**breaking**] introduce ServerConfig
- [**breaking**] eliminate async_trait
- add deprecation warnings to 0.2 branch in preparation for 0.3

### Fixed

- fix tests
- fix deps

### Other

- use path base dev deps only
- use workspace deps to avoid release mistakes
- update changelogs to reflect 1.0.0 versions
- update versions on main to reflect reality
- release 1.0-rc.1 🌱
- replace references to 0.3 with 1.0 in changelogs
- update all changelogs to reflect current status
- some manual clippy fixes
- clippy auto fix
- *(deps)* [**breaking**] update all deps
- edition 2024
- further improvements to format settings
- add a rustfmt.toml and reformat
- release
- clippy
- release
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
- (cargo-release) version 0.0.4
- for clippy
- derive(AsyncRead, AsyncWrite)
- use readme in crate
- (cargo-release) version 0.0.3
- use except
- apparently override is a reserved keyword
- skip → override
- clippy
- add skipping to trillium macros
- syn 2: not the original syn
- (cargo-release) version 0.0.2
- improve macros docs
- clippy
- macros minor feature: add better generics support
- use workspace trillium-testing and trillium
- use map_or_else
- initial commit of trillium-macros

## [0.1.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0

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
