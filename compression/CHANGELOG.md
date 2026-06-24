# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.2] - 2026-06-24

### Fixed
- Client-side `Compression` no longer attempts to decode bodyless responses
  (HEAD, 204, 304). These carry `Content-Encoding` describing the resource
  while having no body of their own; feeding the empty body to a decoder
  surfaced an `UnexpectedEof` error. `Content-Encoding` is now left intact on
  these responses.

## [0.3.1] - 2026-06-02

### Added
- Client-side `Compression` handler behind the new `client` feature
  (`trillium_compression::client::Compression`). Implements `ClientHandler`
  bidirectionally:
  - Responses: advertises `Accept-Encoding` on outbound requests and
    transparently decodes brotli/gzip/zstd response bodies, stripping
    `Content-Encoding` so callers read plaintext.
  - Requests (opt-in): compresses the request body and sets `Content-Encoding`
    when a request encoding is selected — handler-wide via
    `Compression::with_default_encoding`, or per request by setting a
    `CompressionAlgorithm` on the conn's state (which overrides the default).
- `CompressionAlgorithm::Identity`, the identity content-coding. As a per-conn
  client state signal it opts a single request out of a configured default
  request encoding.

## [0.3.0] - 2026-05-08

### Changed (breaking)
- Default brotli level lowered from `Level::Default` (quality 11) to
  `Level::Precise(4)`, matching common reverse-proxy transport defaults.
  Quality 11 is roughly 100× slower than quality 4 for marginal size gains
  and is unsuitable for the response hot path. Callers that want maximum
  compression can opt back in with `with_brotli_level(Level::Best)`.
- Encoders are now constructed with `with_quality(...)` instead of `new()`
  so the configured level is actually applied per request.

### Added
- `with_brotli_level`, `with_gzip_level`, `with_zstd_level` for per-algorithm
  level configuration. `Level` is re-exported from `async-compression`.

### Fixed
- Skip compression when the response already has `Content-Encoding` set.
  Previously the middleware would re-compress precompressed sidecar
  responses, producing invalid output and explosive memory use under any
  concurrency.
- Skip compression for content types that are already compressed (image
  binary formats, video/*, audio/*, web fonts, common archive formats).
  `image/svg+xml` and `application/wasm` are intentionally still compressed.

## [0.2.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0

### Added
- deprecate set_state for insert_state

## [0.1.2](https://github.com/trillium-rs/trillium/compare/trillium-compression-v0.1.1...trillium-compression-v0.1.2) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Other
- release
- release
- *(deps)* update env_logger requirement from 0.10.1 to 0.11.0
- Release only rustls
- release
- release

## [0.1.1](https://github.com/trillium-rs/trillium/compare/trillium-compression-v0.1.0...trillium-compression-v0.1.1) - 2024-01-02

### Other
- deps
- upgrade deps
- remove dependency carats
- Update futures-lite requirement from 1.13.0 to 2.0.0
- Add zstd compression tests
- Add zstd support
- deps
- Update async-compression requirement from 0.3.15 to 0.4.0
- clippy fixes
- patch deps
- Update env_logger requirement from 0.9.0 to 0.10.0
- [static-compiled minor feature] upgrade fork of include_dir
- the paperclip commands me
