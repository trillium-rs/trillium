# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0](https://github.com/trillium-rs/trillium/compare/trillium-logger-v0.4.5...trillium-logger-v0.5.0) - 2026-04-01

### Added

- *(testing)* rename TestHandler to TestServer and misc testing improvements
- update all crates for new style of testing
- [**breaking**] add h3 support to client
- add version to dev formatter
- [**breaking**] remove Conn::inner and Conn::inner_mut
- [**breaking**] introduce ServerConfig
- *(logger)* [**breaking**] remove deprecated formatters::header
- [**breaking**] eliminate async_trait

### Other

- fix up broken docs links
- Add readmes
- update all changelogs to reflect current status
- clippy auto fix
- *(deps)* [**breaking**] update all deps
- edition 2024
- switch over to `///` from `/** */` comments
- further improvements to format settings
- add a rustfmt.toml and reformat
- release

### Changed
- Compatible with trillium 0.3
- `formatters::header()` removed (was deprecated); use `formatters::request_header()` instead
- `dev_formatter` output now includes HTTP version as the first field: format changed from `METHOD URL TIME STATUS` to `VERSION METHOD URL TIME STATUS`

### Added
- `LogTarget` — accessible via `conn.shared_state::<LogTarget>()`, allowing any handler to emit messages to the configured logger target

## [0.4.5](https://github.com/trillium-rs/trillium/compare/trillium-logger-v0.4.4...trillium-logger-v0.4.5) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 0.3

### Other
- release
- release
- clippy
- Release only rustls
- release
- release

## [0.4.4](https://github.com/trillium-rs/trillium/compare/trillium-logger-v0.4.3...trillium-logger-v0.4.4) - 2024-01-02

### Other
- update dependencies other than trillium-rustls
- deps
- upgrade deps
- remove dependency carats
