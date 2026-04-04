# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.7.0](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.6.2...trillium-client-v0.7.0) - 2026-04-04

### Added

- [**breaking**] rename http_config to config
- [**breaking**] rename ServerConfig to HttpContext
- http trailers
- *(testing)* rename TestHandler to TestServer and misc testing improvements
- *(client)* add support for special form request targets
- *(client)* [**breaking**] with_default_pool is now actually the default
- [**breaking**] allow client and api to use sonic-rs instead of serde_json
- further improvements on client and proxy for h3
- [**breaking**] add h3 support to client
- [**breaking**] introduce ServerConfig
- [**breaking**] introduce Runtime
- parse directly into trillium::Headers
- *(client)* [**breaking**] remove deprecated 0.2.x placeholders
- *(client)* [**breaking**] add support for client timeouts
- [**breaking**] eliminate async_trait
- *(http)* [**breaking**] make Upgrade #[non_exhaustive], use Buffer, and add peer_ip
- *(http)* [**breaking**] remove reexported httparse error type

### Fixed

- minimize differences between fieldwork generated methods and previous
- client header finalization for h3
- client now is appropriately factored, uses H3Connection

### Other

- replace references to 0.3 with 1.0 in changelogs
- *(deps)* upgrade async-tungstenite
- fix up broken docs links
- *(client)* Add docs.rs metadata to enable features
- Add readmes
- update all changelogs to reflect current status
- clippy auto fix
- *(deps)* [**breaking**] update all deps
- edition 2024
- switch over to `///` from `/** */` comments
- further improvements to format settings
- add a rustfmt.toml and reformat
- Upgrade thiserror

### Changed
- Compatible with trillium 1.0
- `ObjectSafeConnector` replaced by `ArcedConnector`; `config.arced()` → `ArcedConnector::new(config)`
- Error variants renamed: `MalformedHeader` split into `InvalidHeaderName` and `InvalidHeaderValue`; `PartialHead` merged into `InvalidHead`
- Maximum head size increased from 2KB to 8KB
- Previously deprecated `with_header`, `with_headers`, and `without_header` removed
- `async_trait` re-export removed
- `Client::with_default_pool` removed and keepalive is now the default. To opt out, `Client::without_keepalive` was added

### Added

#### HTTP/3

`Client::new_with_quic(connector, quic_connector)` builds a client with HTTP/3 support. The client tracks `Alt-Svc` response headers and automatically uses HTTP/3 for subsequent requests to origins that advertise it. QUIC connections are pooled; if an H3 attempt fails, that endpoint is marked broken and requests fall back to HTTP/1.1 for a backoff period before retrying. Requests to origins without a cached alt-svc entry always use HTTP/1.1.

`QuicConnector` and `ArcedQuicConnector` are re-exported from `trillium-server-common`. The `QuicConnector` type parameter is bound at construction time (before type erasure), keeping `trillium-quinn` and the runtime adapter as independent crates that neither depends on the other.

```rust
use trillium_client::Client;
use trillium_rustls::RustlsConfig;
use trillium_rustls::rustls::client::ClientConfig;
use trillium_quinn::ClientQuicConfig;

let client = Client::new_with_quic(
    RustlsConfig::<ClientConfig>::default(),
    ClientQuicConfig::with_webpki_roots(),
);
```

#### Other additions

- `Conn::http_version() -> Version` — returns the HTTP version used for the request; after the request completes this reflects whether HTTP/3 was negotiated
- `Client::with_timeout(Duration)` and `Conn::with_timeout(Duration)` — per-request timeouts, returning `Error::TimedOut` on expiry
- `Client::set_timeout(&mut self, Duration)` and `Conn::set_timeout(&mut self, Duration)` — in-place variants of the above
- Per-connection state via `TypeSet`: `with_state`, `insert_state`, `state`, `state_mut`, `take_state`
- `sonic-rs` feature: opt-in alternative to `serde_json` for `with_json_body` and `response_json`. Enable with `features = ["sonic-rs"]`. The two features are mutually exclusive — enable only one. **Note:** unlike `serde_json`, `sonic-rs` does not guarantee stable map key ordering — tests that assert on raw JSON string output may need to parse back to `Value` before comparing. To keep using `serde_json`, use `features = ["serde_json"]`.

## [0.6.2](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.6.1...trillium-client-v0.6.2) - 2024-05-30

### Added
- deprecate Headers::contains_ignore_ascii_case
- *(client)* impl IntoUrl for IpAddr and SocketAddr for convenience

## [0.6.1](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.6.0...trillium-client-v0.6.1) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Fixed
- *(client)* re-add Conn::without_header

## [0.6.0](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.5.6...trillium-client-v0.6.0) - 2024-04-04

### Fixed
- *(client)* [**breaking**] client use of server-common 0.5 was a breaking change

### Other
- release
- release
- release
- clippy
- *(client)* remove references to `with_websocket_upgrade_headers`

## [0.5.6](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.5.5...trillium-client-v0.5.6) - 2024-02-13

### Added
- *(http)* sort Host and Date headers first

### Fixed
- *(client)* set minimum trillium-http version correctly

## [0.5.5](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.5.4...trillium-client-v0.5.5) - 2024-02-05

### Added
- *(client)* fix feature specification

## [0.5.4](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.5.3...trillium-client-v0.5.4) - 2024-01-24

### Fixed
- *(security)* handling of unsafe characters in outbound header names and values

### Other
- *(deps)* update env_logger requirement from 0.10.1 to 0.11.0

## [0.5.3](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.5.2...trillium-client-v0.5.3) - 2024-01-22

### Other
- Make `into_websocket()` send the request if not yet sent
- Rename `websocket` feature to `websockets`
- Add client WebSocket support

## [0.5.2](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.5.1...trillium-client-v0.5.2) - 2024-01-10

### Added
- *(client)* reexport trillium_http::{Body, Method}
- *(client)* reexport ObjectSafeConnector
- *(client)* add Client::connector to borrow the connector
- *(client)* add IntoUrl impls for slices, arrays and vecs

### Other
- Release only rustls
- release
- *(client)* construct Conn directly in Client::build_conn

## [0.5.1](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.5.0...trillium-client-v0.5.1) - 2024-01-02

### Other
- Add tests for using `String` with `IntoUrl`
- `impl IntoUrl for String` for convenience
- use #[test(harness)] instead of #[test(harness = harness)]
- Update test-harness requirement from 0.1.1 to 0.2.0

## [0.5.0](https://github.com/trillium-rs/trillium/compare/trillium-client-v0.4.9...trillium-client-v0.5.0) - 2024-01-02

### Other
- update dependencies other than trillium-rustls
- replace insert_default_header and remove_default_header with default_headers_mut and add default_headers
- client breaking: add default headers and make header access consistent
- introduce IntoUrl and Client::base
- remove ClientLike
- http patch reversion: set Server header before request again
- Avoid an unnecessary to_string() before formatting
- update tests
- client patch: spec compliance improvements
- client patch feature: add Conn::peer_addr
