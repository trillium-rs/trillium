# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.2] - 2026-07-14

### Fixed

- The `Modified` handler — and therefore `CachingHeaders` — no longer answers `304 Not Modified` on
  the strength of `If-Modified-Since` when the request also carries `If-None-Match`. Browsers
  routinely send both, and previously the timestamp comparison could override an entity tag that had
  already determined the representation changed, leaving the client to keep rendering a stale body.
  Such requests are now decided by the entity tag alone, per
  [RFC 9110 §13.1.3](https://www.rfc-editor.org/rfc/rfc9110#section-13.1.3). A request carrying only
  `If-Modified-Since` behaves as before.

  This was most likely to bite responses whose `Last-Modified` is coarser than their etag — one that
  can stay put across a change the etag does capture.

## [0.4.1] - 2026-06-04

### Fixed

- The `Etag` handler now honors `If-None-Match: *`, responding `304 Not Modified` when the response
  carries a representation (a successful status with a body).  Previously the wildcard was parsed as
  an entity-tag, failed, and was silently ignored.

## [0.4.0] - 2026-05-15

### Fixed

- CacheControlDirective::MaxFresh was a typo for the spec directive min-fresh; renamed + accessor +
  parser key.
- Parser previously aborted on unknown directives with non-numeric values
  (e.g. `garbage=non-numeric, max-age=600`); now falls through to UnknownDirective.
- `CacheControlHeader::parse(&str) -> Self` replaces the FromStr impl; parser is now
  infallible. CacheControlParseError removed.
  

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
