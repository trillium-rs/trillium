# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0](https://github.com/trillium-rs/trillium/compare/trillium-aws-lambda-v0.2.2...trillium-aws-lambda-v0.3.0) - 2026-04-04

### Added

- [**breaking**] rename http_config to config
- [**breaking**] rename ServerConfig to HttpContext
- [**breaking**] remove Conn::inner and Conn::inner_mut
- [**breaking**] introduce ServerConfig

### Other

- replace references to 0.3 with 1.0 in changelogs
- Add readmes
- update all changelogs to reflect current status
- some manual clippy fixes
- *(deps)* [**breaking**] update all deps
- edition 2024
- switch over to `///` from `/** */` comments
- release
- release
- release
- release
- *(deps)* update base64 requirement from 0.21.5 to 0.22.0

### Changed
- Compatible with trillium 1.0

## [0.2.2](https://github.com/trillium-rs/trillium/compare/trillium-aws-lambda-v0.2.1...trillium-aws-lambda-v0.2.2) - 2024-01-02

### Other
- updated the following local packages: trillium-http

## [0.2.1](https://github.com/trillium-rs/trillium/compare/trillium-aws-lambda-v0.2.0...trillium-aws-lambda-v0.2.1) - 2024-01-02

### Other
- use trillium-http@v0.3.8
- use trillium-http@v0.3.7
- deps
- bump trillium-http
- upgrade deps
- remove dependency carats
- Update futures-lite requirement from 1.13.0 to 2.0.0
- deps
- (cargo-release) version 0.3.0
- deps
- patch deps
- various nonbreaking dependency updates
- deps
- address base64 deprecation warnings
- Update base64 requirement from 0.20.0 to 0.21.0
- Update base64 requirement from 0.13.1 to 0.20.0
- patch deps
- clippy is my copilot
- aws-lambda patch bugfix: do not transform trillium::Conn into trillium_http::Conn
- deps
- update deps
- cargo upgrade
- 2021
- add caching header support
- update deps
- version bumps
