# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.1](https://github.com/trillium-rs/trillium/compare/trillium-static-v0.5.0...trillium-static-v0.5.1) - 2026-05-10

### Added

- *(static)* range requests
- *(static)* serve precompressed sidecars

### Other

- use workspace deps to avoid release mistakes
- re-release client, runtime adapters, quinn, and rustls

### Added
- HTTP Range request support. Every response advertises `Accept-Ranges:
  bytes`. When a client sends a single-range `Range: bytes=...` header
  (`START-END`, `START-`, or `-SUFFIX`), the handler seeks into the file
  and streams just that byte range with status `206 Partial Content`,
  `Content-Range`, and `Content-Length`. Out-of-bounds ranges return `416
  Requested Range Not Satisfiable` with `Content-Range: bytes */N`.
  Multi-range requests fall through to a `200` full body. Honors
  `If-Range` (strong-comparison only per RFC 9110); the metadata-derived
  etag is weak and so will not satisfy `If-Range`, but the
  `Last-Modified` date will. Ranged requests bypass precompressed-sidecar
  selection — the range applies to the identity representation, never to
  a compressed sidecar.

- Precompressed-sidecar serving. `StaticFileHandler::with_precompressed()`
  serves `<asset>.br`, `<asset>.zst`, and `<asset>.gz` siblings when the
  client's `Accept-Encoding` allows them, in that priority order, with
  `Content-Encoding` set and the original asset's MIME type preserved.
  `Vary: Accept-Encoding` is emitted on every response from the handler
  while the feature is enabled — including the uncompressed-original
  fallback — so caches do not serve a compressed response to a client
  that did not ask for one. Composes with `trillium-compression`, which
  passes any response that already has `Content-Encoding` through
  unchanged. For non-default codings or suffixes, register variants
  individually with `with_precompressed_variant(encoding, suffix)`.

### Fixed
- `.without_etag_header()` and `.without_modified_header()` now also apply
  to direct-file requests, not just to index-file resolution.

## [0.5.0] - 2026-05-02

### Fixed
- path traversal issues on windows ([#754](https://github.com/trillium-rs/trillium/pull/754))

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

## [0.4.1](https://github.com/trillium-rs/trillium/compare/trillium-static-v0.4.0...trillium-static-v0.4.1) - 2024-01-02

### Fixed
- fix failing docs build for testing & static

### Other
- update dependencies other than trillium-rustls
- deps
- upgrade deps
- remove dependency carats
- Update futures-lite requirement from 1.13.0 to 2.0.0
- Update async-fs requirement from 1.6.0 to 2.0.0
- deps
- various nonbreaking dependency updates
- deps
- clippy fixes
- Update etag requirement from 3.0.0 to 4.0.0
- patch deps
- [static patch feature] add some tracing
- update deps
- better error message for crates that require a runtime
- Revert "allow !Send initialization" -- this was not intended to be on main
- allow !Send initialization
- cargo upgrade
- 2021
