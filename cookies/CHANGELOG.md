# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.1] - 2026-05-15

### Added: Client Cookie Jar

- New optional `client` cargo feature exposing `trillium_cookies::client::Cookies`, a
  `ClientHandler` for `trillium-client` that maintains an RFC 6265 cookie jar across
  requests issued by the same `Client`. Outbound requests pick up matching cookies on the
  `Cookie` header; inbound responses' `Set-Cookie` headers are parsed and stored. Domain,
  path, expiration, and `Secure` semantics are delegated to `cookie_store::CookieStore`.

  Seed an existing jar via `Cookies::with_store(cookie_store)` and inspect or serialize
  via `Cookies::borrow(|store| ...)`. The `cookie_store` crate is re-exported as
  `trillium_cookies::client::cookie_store` so callers don't need a separate dependency.

  Additional `client-serialization` cargo feature turns on `cookie_store/serde` for
  save/load of jar snapshots.

## [0.5.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Other
- release
- release
- *(deps)* update env_logger requirement from 0.10.1 to 0.11.0
- Release only rustls
- release
- release

## [0.4.1](https://github.com/trillium-rs/trillium/compare/trillium-cookies-v0.4.0...trillium-cookies-v0.4.1) - 2024-01-02

### Other
- upgrade deps
- remove dependency carats
- cookie minor: update cookie dependency
- deps
