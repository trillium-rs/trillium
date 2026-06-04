# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.1] - 2026-06-01

### Added

- Opt-in `405 Method Not Allowed` handling via `Router::with_method_not_allowed()` (and
  `RouterRef::set_method_not_allowed`). When enabled, a request whose path matches a route but whose
  method does not receives a `405` with an `Allow` header listing the path's supported methods —
  the natural sibling of the existing default-on OPTIONS handling. The status is set *without
  halting the conn*, so a later handler can replace it; the `405` only stands on true fall-through.
  Opt-in because it changes responses that previously fell through (typically `404`) and because
  advertising supported methods reveals that the path exists.

## [0.5.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0

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
