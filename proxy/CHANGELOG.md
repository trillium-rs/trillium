# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.8.4] - 2026-07-04

### Fixed
- A request body on an HTTP/2 or HTTP/3 request that carries no `Content-Length` (framed by
  `END_STREAM` / DATA) is no longer silently dropped. Body presence was previously detected by
  sniffing the `Content-Length` / `Transfer-Encoding` headers, which an h2/h3 body need not set;
  the request body is now forwarded unconditionally and the client frames it.

## [0.8.3] - 2026-07-02

### Fixed
- `Proxy::without_halting` now applies to the `502 Bad Gateway` responses produced when the upstream
  connection fails or returns no status. Previously these branches halted the conn unconditionally,
  contradicting the documented behavior and preventing a subsequent handler from replacing the error
  with a user-facing body.

## [0.8.2] - 2026-07-01

### Fixed
- **Security:** a request path whose first segment contains a colon (e.g. `/trillium::Handler`) or
  that embeds an absolute url (e.g. `/https://elsewhere.example`) is now joined onto the configured
  upstream as a path. Previously such a path was resolved as an absolute url: an embedded absolute
  url could redirect the request to an attacker-controlled host, and other colon-first paths failed
  to proxy at all.
- **Security:** a request path can no longer use `..` traversal to escape a `Url` upstream's base
  path prefix. A proxy mounted at e.g. `http://backend/app/` previously forwarded `/../secret` to
  `http://backend/secret`, walking out of the intended mount; such requests are now left unproxied
  and pass through to the next handler. Percent-encoded traversal (`%2e%2e`) is refused the same way.
- A `Url` upstream configured without a trailing slash (e.g. `http://backend/api`) now keeps its
  path as a mount prefix — a request for `/users` proxies to `http://backend/api/users`. Previously
  the final segment was treated as a file and replaced, dropping the prefix (`http://backend/users`).
- Upstream connection or response failures now return `502 Bad Gateway` instead of `503 Service
  Unavailable`.

## [0.8.1] - 2026-06-21

### Fixed
- Hop-by-hop headers named in a `Connection` header that is split across multiple request header
  lines are now stripped before forwarding upstream; previously a multi-line `Connection` header was
  ignored and the headers it named leaked through.

## [0.8.0] - 2026-05-15

### Changed
- compatible with trillium-client 0.9

## [0.7.0] - 2026-05-05

### Changed
- compatible with trillium-client 0.8 and trillium 1.1

## [0.6.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0

### Fixed

- *(proxy)* Inappropriately stripped Connection / Upgrade in SwitchingProtocol

## [0.5.5](https://github.com/trillium-rs/trillium/compare/trillium-proxy-v0.5.4...trillium-proxy-v0.5.5) - 2024-05-30

### Added
- deprecate set_state for insert_state

## [0.5.4](https://github.com/trillium-rs/trillium/compare/trillium-proxy-v0.5.3...trillium-proxy-v0.5.4) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Other
- release
- release
- release
- release

## [0.5.3](https://github.com/trillium-rs/trillium/compare/trillium-proxy-v0.5.2...trillium-proxy-v0.5.3) - 2024-03-22

### Fixed
- *(proxy)* use Connector and ObjectSafeConnector from trillium_client

### Other
- clippy
- *(clippy)* fix two clippies
- *(deps)* update env_logger requirement from 0.10.1 to 0.11.0
- release

## [0.5.2](https://github.com/trillium-rs/trillium/compare/trillium-proxy-v0.5.1...trillium-proxy-v0.5.2) - 2024-01-02

### Other
- updated the following local packages: trillium-http, trillium-server-common, trillium-client

## [0.5.1](https://github.com/trillium-rs/trillium/compare/trillium-proxy-v0.5.0...trillium-proxy-v0.5.1) - 2024-01-02

### Other
- whatever clippy wants, clippy gets
- use pooling in example
- proxy patch: only stream the request body if it's present, and handle get requests with bodies
- proxy patch bugfix: forward proxy connect needs to halt
