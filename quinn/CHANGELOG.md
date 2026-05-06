# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] - 2026-XX-XX

### Added
- `pub use quinn` — the underlying `quinn` crate is now re-exported at the crate root, so callers don't need to add `quinn` as a separate dependency to interact with the underlying QUIC types

## [0.1.0] - 2026-05-02

### Added
- Initial release: Quinn-backed QUIC adapter for HTTP/3 support
