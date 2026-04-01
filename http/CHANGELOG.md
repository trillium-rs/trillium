# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0](https://github.com/trillium-rs/trillium/compare/trillium-http-v0.3.17...trillium-http-v0.4.0) - 2026-04-01

### Added

- small perf improvements
- http trailers
- add some more known header names
- *(testing)* rename TestHandler to TestServer and misc testing improvements
- update all crates for new style of testing
- *(client)* add support for special form request targets
- drop `Sync` found from Body::new_streaming
- further improvements on client and proxy for h3
- [**breaking**] add h3 support to client
- [**breaking**] webtransport and some tidying of h3/quic
- auto-populate alt-svc response header
- [**breaking**] hypertext transfer protocol, three
- [**breaking**] remove Conn::inner and Conn::inner_mut
- [**breaking**] introduce ServerConfig
- *(http)* [**breaking**] remove deprecated Headers::contains_ignore_ascii_case
- add Headers::entry interface
- [**breaking**] use the extracted `type-set` crate instead of trillium_http::StateSet
- [**breaking**] introduce Runtime
- parse directly into trillium::Headers
- [**breaking**] use swansong instead of stopper + clone counter
- *(client)* [**breaking**] add support for client timeouts
- *(http)* [**breaking**] support !Send handler functions
- [**breaking**] eliminate async_trait
- *(http)* [**breaking**] remove Stream for ReceivedBody
- *(http)* [**breaking**] make Upgrade #[non_exhaustive], use Buffer, and add peer_ip
- *(http)* [**breaking**] remove reexported httparse error type
- *(http)* [**breaking**] synthetic len returns usize
- *(http)* add Conn::shared_state
- *(http)* add StateSet::merge

### Fixed

- address failing test
- address some straggling security/correctness issues in 0.3
- client now is appropriately factored, uses H3Connection
- nonparse
- update tests to reflect how Server header is set now
- http trillium-macros dep version

### Other

- resolve clippy
- fix up broken docs links
- *(http)* use a nibble-based huffman decoder instead of bitwise
- clippy fixes in http
- Add readmes
- update all changelogs to reflect current status
- some manual clippy fixes
- clippy auto fix
- *(deps)* [**breaking**] update all deps
- mv conn/implementation to conn/h1
- readd FrameHeader::encode, which was used by tests
- documentation pass
- edition 2024
- switch over to `///` from `/** */` comments
- further improvements to format settings
- add a rustfmt.toml and reformat
- add two more corpus tests
- add httparse tests and make errors identical
- Upgrade thiserror

### Changed
- Compatible with trillium 0.3
- `StateSet` renamed to `TypeSet` and extracted to the [`type-set`](https://docs.rs/type-set) crate; re-exported as `trillium_http::TypeSet`
- Trillium 0.3 uses [Swansong](https://docs.rs/swansong) instead of Stopper; `Conn::stopper()` → `Conn::swansong()`
- `Error` variants renamed for consistency: `MalformedHeader` → `InvalidHeaderValue`, `PartialHead` → `InvalidHead`, `MissingVersion` → `InvalidVersion`, `UnrecognizedStatusCode`/`MissingStatusCode` → `InvalidStatus`/`MissingStatus`; `HeaderMissing` and `UnexpectedHeader` now carry `HeaderName<'static>` instead of `&'static str`; `UnsupportedVersion` now carries `Version` instead of `u8`
- `Version::Http2_0` renamed to `Version::Http2`; `Version::Http3_0` renamed to `Version::Http3`
- `Upgrade` is now `#[non_exhaustive]`; `Upgrade::buffer` changed from `Option<Vec<u8>>` to `Buffer`; `Upgrade::stopper` renamed to `Upgrade::swansong`; `Upgrade::peer_ip: Option<IpAddr>` added
- `ReceivedBody` no longer implements `Stream`; use `AsyncRead` instead
- `Headers::contains_ignore_ascii_case` removed (was deprecated)
- `Headers::append` and `Headers::try_insert_with` now return `&mut HeaderValues` instead of `()`
- `set_*` setters on `Conn` (e.g. `set_status`, `set_host`) now return `&mut Self`, enabling chaining
- Handler futures in `Conn::map` and friends no longer require `Send`
- `pub mod transport` removed — the `Transport` trait is now at `trillium::Transport`; `BoxedTransport` remains as a type alias
- `Body::new_streaming` no longer requires a `Sync` reader.

### Added
- `Headers::entry()` — Entry API for inserting/modifying headers, mirroring `HashMap::entry`
- `parse` feature — opt-in alternative header parser (bypasses httparse; groundwork for H3)
- `ServerConfig` is now public — Arc-shared per-server state (Swansong + TypeSet + HttpConfig) passed to every connection
- `pub mod h3` — HTTP/3 protocol primitives: QPACK encode/decode, H3 framing, `H3Connection`, `H3Body`, `H3Error`; used by [`trillium-quinn`](https://docs.rs/trillium-quinn) and other QUIC adapter crates

## [0.3.17](https://github.com/trillium-rs/trillium/compare/trillium-http-v0.3.16...trillium-http-v0.3.17) - 2024-05-30

### Added
- deprecate Headers::contains_ignore_ascii_case

## [0.3.16](https://github.com/trillium-rs/trillium/compare/trillium-http-v0.3.15...trillium-http-v0.3.16) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 0.3

### Fixed
- remove unreleased Upgrade::request_headers and Upgrade::request_headers_mut
- *(trillium)* fix the flaky liveness test

## [0.3.15](https://github.com/trillium-rs/trillium/compare/trillium-http-v0.3.14...trillium-http-v0.3.15) - 2024-03-22

### Added
- *(http)* sort Host and Date headers first
- *(test)* add corpus tests

### Other
- clippy
- *(http)* document addition of is_valid

## [0.3.14](https://github.com/trillium-rs/trillium/compare/trillium-http-v0.3.13...trillium-http-v0.3.14) - 2024-02-08

### Added
- *(http)* add the notion of closure to synthetic bodies

### Fixed
- *(http)* fix Conn::is_disconnected logic
- *(http)* fix synthetic body AsyncRead implementation for large bodies

## [0.3.13](https://github.com/trillium-rs/trillium/compare/trillium-http-v0.3.12...trillium-http-v0.3.13) - 2024-02-05

### Added
- *(http)* fix http-compat cargo feature specification
- *(http)* relax handler constraint to be FnMut instead of Fn
- *(http)* cancel on disconnect

### Other
- *(http)* appease the clipmaster

## [0.3.12](https://github.com/trillium-rs/trillium/compare/trillium-http-v0.3.11...trillium-http-v0.3.12) - 2024-01-24

### Fixed
- *(security)* allow all tchar in header names
- *(security)* handling of unsafe characters in outbound header names and values

### Other
- *(deps)* update env_logger requirement from 0.10.1 to 0.11.0

## [0.3.11](https://github.com/trillium-rs/trillium/compare/trillium-http-v0.3.10...trillium-http-v0.3.11) - 2024-01-02

### Other
- use #[test(harness)] instead of #[test(harness = harness)]
- Update test-harness requirement from 0.1.1 to 0.2.0

## [0.3.10](https://github.com/trillium-rs/trillium/compare/trillium-http-v0.3.9...trillium-http-v0.3.10) - 2024-01-02

### Other
- Update smol requirement from 1.3.0 to 2.0.0
- update dependencies other than trillium-rustls
- http patch reversion: set Server header before request again
- http patch feature: serde for version
- http patch: don't send explicit connection: keep-alive
