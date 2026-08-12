# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.0] - 2026-08-12

### Changed
- **Breaking**: Upgraded to tera 2.0. `tera::Tera`, `tera::Context`, and the
  filter/function/test traits are reexported from tera 2, so any custom filters,
  functions, or tests need to be updated for tera's new signatures. `tera::Result`
  is now `tera::TeraResult`.
- **Breaking**: `TeraConnExt::assign` now takes `impl Into<Cow<'static, str>>` for
  the key instead of `&str`, mirroring tera 2's `Context::insert`. String literals
  continue to work unchanged; non-`'static` keys need an explicit `.to_string()`.

### Added
- Reexported `tera::Error` and `tera::TeraResult`.
- Passthrough features for tera's `fast` (on by default, per tera's
  recommendation), `preserve_order`, and `unicode`.

## [0.4.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0

### Other
- *(askama,tera)* move from mime-db to mime_guess
- Release only rustls
- release
- release

## [0.3.1](https://github.com/trillium-rs/trillium/compare/trillium-tera-v0.3.0...trillium-tera-v0.3.1) - 2024-01-02

### Fixed
- fix suddenly failing tera tests (??)

### Other
- upgrade deps
- remove dependency carats
- deps
- deps
- patch deps
- deps
- patch deps
- clippy is my copilot
