# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.0] - 2026-09-01

### Changed

- Client disconnection now drops the event stream promptly instead of waiting for the next write to
  fail. A stream backed by a subscription can rely on `Drop` to unsubscribe.
- **Breaking:** `SseConnExt` is removed, along with `with_sse_stream` and
  `with_sse_stream_and_heartbeat`. The `Sse` handler is the one way to serve an event stream;
  anywhere `with_sse_stream` was called inside a handler, mount `sse(|conn: &mut Conn| ...)`
  instead, and replace the heartbeat variant with `Sse::with_heartbeat`.

## [0.3.0] - 2026-08-06

### Added

- Comment messages, the standard SSE heartbeat. `Eventable::comment` returns an optional
  comment to send alongside (or instead of) data, and `Event::new_comment`/`with_comment`/
  `set_comment` build them.
- `Event` can now carry an `id`, via `with_id`/`set_id`/`id`. `Eventable::id` already existed
  but `Event` had no way to populate it.
- `Event` implements `Default`.
- `retry:` support, for telling clients how long to wait before reconnecting. `Eventable::retry`
  returns an optional `Duration`, emitted in milliseconds; `Event::with_retry`/`set_retry` set it.

- An `Sse` handler, built with `sse()` from any `SseHandler`. Unlike `SseConnExt`, it is a
  `Handler`, so it obtains a runtime in `init` and can send heartbeat comments via
  `Sse::with_heartbeat`. `SseHandler` is implemented for any `Fn(&mut Conn) -> Stream`. The
  handler also negotiates on `Accept`, passing requests that exclude `text/event-stream`
  through to subsequent handlers.
- `SseConnExt::with_sse_stream_and_heartbeat`, which sends an empty comment whenever the given
  interval elapses without the stream yielding an event. The interval is measured from the most
  recent event, so a busy stream sends no heartbeats.

### Changed

- The stream passed to `with_sse_stream` no longer needs to be `Sync`, only `Send`. This admits
  channel receivers that were previously rejected.
- **Breaking:** `Eventable::data` returns `Option<&str>` instead of `&str`, so that a message can
  be sent with no `data:` field. `Event::data` likewise.
- Data and comment values containing empty lines now emit a bare `data:`/`:` line for each,
  rather than dropping them.
- An `Eventable` with no fields set at all is skipped rather than written as a bare message
  terminator.

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
