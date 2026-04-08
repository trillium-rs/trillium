# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0-rc.13](https://github.com/trillium-rs/trillium/compare/trillium-api-v0.2.0-rc.12...trillium-api-v0.2.0-rc.13) - 2026-04-08

### Added

- normalization pass
- [**breaking**] Conn::request_body does not require an await.
- *(testing)* rename TestHandler to TestServer and misc testing improvements
- *(api)* remove default features
- update all crates for new style of testing
- [**breaking**] allow client and api to use sonic-rs instead of serde_json
- [**breaking**] fix up the straggling -> () setters
- [**breaking**] introduce ServerConfig
- [**breaking**] eliminate async_trait
- [**breaking**] make all conn header apis specify request or response

### Other

- replace references to 0.3 with 1.0 in changelogs
- *(deps)* upgrade async-tungstenite
- fix up broken docs links
- Add readmes
- document trillium-api
- update all changelogs to reflect current status
- *(deps)* [**breaking**] update all deps
- *(api)* small updates for 0.3
- edition 2024
- tidying
- switch over to `///` from `/** */` comments
- further improvements to format settings
- add a rustfmt.toml and reformat
- fix disconnect-body test
- Upgrade thiserror

### Changed
- Compatible with trillium 1.0
- `FromConn` and `TryFromConn` no longer use `#[async_trait]`; remove the attribute from any implementations in your code
- `sonic-rs` is now the default JSON library, replacing `serde_json`. The two are mutually exclusive features — `sonic-rs` is active by default. If `serde_json` is already a direct dependency in your project, you can keep it via `default-features = false, features = ["forms", "serde_json"]`; otherwise we recommend switching for substantial JSON parsing performance improvements. **Note:** unlike `serde_json`, `sonic-rs` does not guarantee stable map key ordering — tests that assert on raw JSON string output may need to parse back to `Value` before comparing.

### Added
- `impl FromConn for trillium_http::Version` — HTTP version is now extractable as an API handler parameter

## [0.2.0-rc.12](https://github.com/trillium-rs/trillium/compare/trillium-api-v0.2.0-rc.11...trillium-api-v0.2.0-rc.12) - 2024-05-30

### Added
- *(api)* [**breaking**] make IoErrors respond with BadRequest
- *(api)* [**breaking**] implement TryFromConn for `Vec<u8>` and `String`

## [0.2.0-rc.11](https://github.com/trillium-rs/trillium/compare/trillium-api-v0.2.0-rc.10...trillium-api-v0.2.0-rc.11) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Other
- release
- release
- clippy

## [0.2.0-rc.10](https://github.com/trillium-rs/trillium/compare/trillium-api-v0.2.0-rc.9...trillium-api-v0.2.0-rc.10) - 2024-02-13

### Fixed
- *(api)* set minimum trillium dependency correctly

## [0.2.0-rc.9](https://github.com/trillium-rs/trillium/compare/trillium-api-v0.2.0-rc.8...trillium-api-v0.2.0-rc.9) - 2024-02-05

### Added
- *(api)* add cancel_on_disconnect

### Other
- *(deps)* update env_logger requirement from 0.10.1 to 0.11.0
- Release only rustls
- release
- release

## [0.2.0-rc.8](https://github.com/trillium-rs/trillium/compare/trillium-api-v0.2.0-rc.7...trillium-api-v0.2.0-rc.8) - 2024-01-02

### Other
- update dependencies other than trillium-rustls
- deps
- upgrade deps
