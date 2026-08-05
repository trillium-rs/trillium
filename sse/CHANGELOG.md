# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Comment messages, the standard SSE keep-alive. `Eventable::comment` returns an optional
  comment to send alongside (or instead of) data, and `Event::new_comment`/`with_comment`/
  `set_comment` build them.
- `Event` can now carry an `id`, via `with_id`/`set_id`/`id`. `Eventable::id` already existed
  but `Event` had no way to populate it.
- `Event` implements `Default`.

### Changed

- **Breaking:** `Eventable::data` returns `Option<&str>` instead of `&str`, so that a message can
  be sent with no `data:` field. `Event::data` likewise. An `Eventable` with neither data nor a
  comment is skipped rather than written as an empty frame.
- Data and comment values containing empty lines now emit a bare `data:`/`:` line for each,
  rather than dropping them.

## [0.2.1] - 2026-06-21

### Fixed

- Event streams are sent with close-delimited framing (`Connection: close`) rather than chunked
  transfer-encoding. The server-sent-events specification cautions that chunking can interfere with
  event delivery timing.

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

## [0.1.1](https://github.com/trillium-rs/trillium/compare/trillium-sse-v0.1.0...trillium-sse-v0.1.1) - 2024-01-02

### Other
- deps
- 📎💬
- upgrade deps
- remove dependency carats
- Update futures-lite requirement from 1.13.0 to 2.0.0
- deps
- clippy fixes
- clippy is my copilot
- [static-compiled minor feature] upgrade fork of include_dir
- the paperclip commands me
