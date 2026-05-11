# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-05-15

### Added: Client Handler

- New optional `client` cargo feature exposing `trillium_redirect::client::FollowRedirects`,
  a `ClientHandler` for `trillium-client` that automatically follows 301/302/303/307/308
  responses, re-issuing the request through the same client.

  Defaults: up to 10 redirects, HTTPS-to-HTTP downgrades blocked, all origins allowed.
  Tunable via `with_max_redirects`, `with_allow_downgrade`, and `with_allowed_origins`.

  Method handling follows the standard browser convention: 301 and 302 demote POST to GET,
  303 always becomes GET, 307 and 308 preserve the method. Static request bodies replay
  across redirects; streaming bodies are one-shot, and the redirected request is sent
  without a body. Cross-origin redirects strip `Authorization`, `Cookie`, and
  `Proxy-Authorization` headers.

  Failures (`RedirectError`) cover too-many-redirects, disallowed origin, blocked
  downgrade, missing `Location`, and unparseable `Location`.

## [0.2.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Other
- release
- release
- Release only rustls
- release
- release

## [0.1.1](https://github.com/trillium-rs/trillium/compare/trillium-redirect-v0.1.0...trillium-redirect-v0.1.1) - 2024-01-02

### Other
- remove dependency carats
