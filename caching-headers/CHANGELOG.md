# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.0] - 2026-05-15

### Fixed

- CacheControlDirective::MaxFresh was a typo for the spec directive
  min-fresh (RFC 9111 §5.2.1.3); rename + accessor + parser key.
- Parser previously aborted on unknown directives with non-numeric
  values (e.g. `garbage=non-numeric, max-age=600`); now falls through
  to UnknownDirective per RFC 9111 §5.2.
- CacheControlHeader::parse(&str) -> Self replaces the FromStr impl;
  parser is now infallible. CacheControlParseError removed.
  

## [0.3.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Other
- release
- release
- clippy
- Release only rustls
- release
- release

## [0.2.2](https://github.com/trillium-rs/trillium/compare/trillium-caching-headers-v0.2.1...trillium-caching-headers-v0.2.2) - 2024-01-02

### Other
- upgrade deps
- remove dependency carats
