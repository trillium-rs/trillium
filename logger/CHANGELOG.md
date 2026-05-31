# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.3] - 2026-05-31

### Changed

- The startup announcement now enumerates every bound listener via `Info::listeners()` instead of only the primary TCP/Unix address. A TCP-TLS listener and a QUIC listener on the same address collapse onto one line marked `(h3)`; QUIC on a distinct address is listed separately. The canonical `url` line (which can differ from any bound address) is still shown when present.

## [0.5.2] - 2026-05-26

### Added

- `Logger::without_init_message`

## [0.5.1] - 2026-05-15

### Added: Client Logger

- New optional `client` cargo feature exposing `trillium_logger::client::ClientLogger`, a
  `ClientHandler` for `trillium-client` that emits one log line per request. Formatters
  compose from primitives in `trillium_logger::client::formatters` (method, url, status,
  version, response time, transport error, timestamp, request/response header, content
  length, scheme indicator); `ClientLogFormatter` is implemented for tuples, `&'static str`,
  `Arc<str>`, and `Fn(&Conn, bool) -> impl Display`.

  Defaults: `dev_formatter`, `ColorMode::Auto`, `Target::Stdout`. Tunable via
  `with_formatter`, `with_color_mode`, and `with_target`.

  Log lines cover every request — successful responses, responses synthesized by an
  upstream handler (cache hit, mock), and transport-layer failures (connection refused,
  TLS error, timeout). The default `dev_formatter` renders any transport error inline.

## [0.5.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0
- `formatters::header()` removed (was deprecated); use `formatters::request_header()` instead
- `dev_formatter` output now includes HTTP version as the first field: format changed from `METHOD URL TIME STATUS` to `VERSION METHOD URL TIME STATUS`

### Added
- `LogTarget` — accessible via `conn.shared_state::<LogTarget>()`, allowing any handler to emit messages to the configured logger target

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Other
- release
- release
- clippy
- Release only rustls
- release
- release

## [0.4.4](https://github.com/trillium-rs/trillium/compare/trillium-logger-v0.4.3...trillium-logger-v0.4.4) - 2024-01-02

### Other
- update dependencies other than trillium-rustls
- deps
- upgrade deps
- remove dependency carats
