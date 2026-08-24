# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.6.0] - 2026-08-24

### Fixed
- A session store that cannot be reached no longer reads as "this visitor has no session". The
  request now halts with a `503 Service Unavailable` by default, because proceeding minted a
  replacement session and overwrote the cookie, orphaning the session the visitor actually had
  and logging them out permanently rather than for the duration of the outage. Pass `()` to
  `SessionHandler::with_store_error_handler` for the previous behavior.

### Added
- `SessionHandler::with_store_error_handler` runs a handler of your choosing when the session
  store cannot be reached. If it halts, the request ends there; if not, the request proceeds with
  an empty session.
- `SessionStoreError` and `SessionConnExt::session_store_error`.

## [0.5.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Other
- release
- release
- clippy
- *(deps)* update env_logger requirement from 0.10.1 to 0.11.0
- Release only rustls
- release
- release

## [0.4.3](https://github.com/trillium-rs/trillium/compare/trillium-sessions-v0.4.2...trillium-sessions-v0.4.3) - 2024-01-02

### Other
- upgrade deps
- remove dependency carats
- sessions minor: update cookies dependency
- deps
