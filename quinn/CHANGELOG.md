# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.3](https://github.com/trillium-rs/trillium/compare/trillium-quinn-v0.1.2...trillium-quinn-v0.1.3) - 2026-05-10

### Other

- updated the following local packages: trillium-macros

## [0.1.2] - 2026-05-05

### Fixed
- Bump `trillium-server-common` dependency specifier to `0.7` to match the 1.1 release; `0.1.1` was published with a stale `0.6` spec.

## [0.1.1] - 2026-05-05 [YANKED]

### Added
- `pub use quinn` — the underlying `quinn` crate is now re-exported at the crate root, so callers don't need to add `quinn` as a separate dependency to interact with the underlying QUIC types

## [0.1.0] - 2026-05-02

### Added
- Initial release: Quinn-backed QUIC adapter for HTTP/3 support
