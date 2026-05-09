# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.7.0]

### Added
- Compile-time entity-tag computation, on by default. The macro hashes each
  file's source bytes via `etag::EntityTag::from_data` and bakes the
  resulting tag string as a `&'static str`; the handler emits it as the
  `ETag` response header (one tag per source, applied to all encodings).
  Opt out per invocation with `static_compiled!("./files", etag = false)`.
  The baked tag is byte-identical to what `trillium_caching_headers::Etag`
  would compute at runtime, so chaining `Etag::new()` after this handler
  composes naturally — that handler observes the precomputed tag, skips
  rehashing the body, and handles `If-None-Match` / `304 Not Modified`.
- Compile-time precompression of file contents into Brotli, Zstd, and Gzip
  variants, gated behind opt-in cargo features (`brotli`, `zstd`, `gzip`,
  or the `compression` meta-feature) and an opt-in macro argument:
  `static_compiled!("./files", compress)` bakes every variant whose feature
  is enabled, and `static_compiled!("./files", compress = [Brotli, Gzip])`
  bakes a specified subset. Encoders run at maximum quality in parallel via
  rayon. Per-file variants are sorted smallest-first, and only baked when
  they beat the source by at least 5%; files under 256 bytes are skipped
  entirely. The handler picks the smallest variant the client's
  `Accept-Encoding` allows, sets `Content-Encoding`, and emits
  `Vary: Accept-Encoding` (per-file, only when variants are baked).
  Composes with `trillium-compression`, which passes through any response
  that already has `Content-Encoding` set.
- Public `Encoding` enum with `Encoding::token()` returning the HTTP
  content-coding token, and `File::with_encodings`, `File::encodings`, and
  `File::pick_encoding` for inspecting baked variants.

## [0.6.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0

## [0.5.2](https://github.com/trillium-rs/trillium/compare/trillium-static-compiled-v0.5.1...trillium-static-compiled-v0.5.2) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Other
- release
- release
- Release only rustls
- release
- release

## [0.5.1](https://github.com/trillium-rs/trillium/compare/trillium-static-compiled-v0.5.0...trillium-static-compiled-v0.5.1) - 2024-01-02

### Other
- upgrade deps
- remove dependency carats
- deps
- clippy fixes
- patch deps
- clippy pass
- deps
