# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/trillium-rs/trillium/compare/trillium-basic-auth-v0.1.1...trillium-basic-auth-v0.2.0) - 2026-04-01

### Added

- *(testing)* rename TestHandler to TestServer and misc testing improvements
- update all crates for new style of testing
- [**breaking**] eliminate async_trait

### Other

- Add readmes
- update all changelogs to reflect current status
- *(deps)* [**breaking**] update all deps
- edition 2024
- switch over to `///` from `/** */` comments

### Changed
- Compatible with trillium 0.3

## [0.1.1](https://github.com/trillium-rs/trillium/compare/trillium-basic-auth-v0.1.0...trillium-basic-auth-v0.1.1) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 0.3

### Other
- *(deps)* update base64 requirement from 0.21.5 to 0.22.0
