# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0](https://github.com/trillium-rs/trillium/compare/trillium-router-v0.4.1...trillium-router-v0.5.0) - 2026-04-04

### Added

- *(testing)* rename TestHandler to TestServer and misc testing improvements
- update all crates for new style of testing
- [**breaking**] hypertext transfer protocol, three
- [**breaking**] eliminate async_trait

### Other

- replace references to 0.3 with 1.0 in changelogs
- *(deps)* upgrade async-tungstenite
- remove some straggling dbg! macros
- Add readmes
- update all changelogs to reflect current status
- some manual clippy fixes
- clippy auto fix
- *(deps)* [**breaking**] update all deps
- edition 2024
- switch over to `///` from `/** */` comments
- further improvements to format settings
- add a rustfmt.toml and reformat
- release

### Changed
- Compatible with trillium 1.0

## [0.4.1](https://github.com/trillium-rs/trillium/compare/trillium-router-v0.4.0...trillium-router-v0.4.1) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Other
- release

## [0.4.0](https://github.com/trillium-rs/trillium/compare/trillium-router-v0.3.6...trillium-router-v0.4.0) - 2024-03-22

### Added
- *(router)* [**breaking**] fully remove memchr feature
- *(router)* enable "memchr" feature by default
- *(router)* [**breaking**] remove routefinder types from the public api
- *(router)* expose the routefinder memchr feature

### Other
- clippy
- *(router)* remove unused import
- *(deps)* update env_logger requirement from 0.10.1 to 0.11.0
- Release only rustls
- release
- release

## [0.3.6](https://github.com/trillium-rs/trillium/compare/trillium-router-v0.3.5...trillium-router-v0.3.6) - 2024-01-02

### Other
- upgrade deps
- remove dependency carats
- deps
