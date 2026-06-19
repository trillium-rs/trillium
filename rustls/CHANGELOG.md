# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.11.4](https://github.com/trillium-rs/trillium/compare/trillium-rustls-v0.11.3...trillium-rustls-v0.11.4) - 2026-06-19

### Other

- updated the following local packages: trillium-server-common

## [0.11.3] - 2026-06-16

### Added

- The connector now implements `Connector::connect_to` (new in `trillium-server-common`): the
  pre-resolved addresses carried in the `Destination` are forwarded to the inner connector for the
  TCP dial, while the TLS server name still comes from the destination host — so address-pinned
  dialing works over TLS without affecting certificate validation.
- A non-empty per-connection ALPN list (`Destination::alpn`) overrides the configured ALPN protocol
  list for that single connection; an empty list (the default) leaves the connector's configured
  ALPN in place.

## [0.11.2] - 2026-05-26

### Added
- `RustlsClientConfig::from_root_cert_pem(pem)` — build a client config that trusts exactly the certificate(s) in the provided PEM (ignoring platform/webpki defaults) while keeping certificate verification intact. Useful for connecting to a service with a private or self-signed certificate without reconstructing the crate's provider/ALPN defaults by hand.
- `RustlsClientConfig` is now re-exported from the crate root.
- `dangerous` cargo feature, gating `RustlsClientConfig::dangerously_accept_any_cert()` — a client config that disables server authentication entirely.

### Fixed
- Connecting over TLS to a host given as an IP address (e.g. `https://127.0.0.1`) failed with a `missing domain` transport error; only DNS hostnames worked. IP-address hosts now connect, validated against the certificate's IP SAN (no SNI is sent for them, per the TLS spec).

## [0.11.1] - 2026-05-05

### Fixed
- Bump `trillium-server-common` dependency specifier to `0.7` to match the 1.1 release; `0.11.0` was published with a stale `0.6` spec.

## [0.11.0] - 2026-05-05 [YANKED]

### Changed
- TLS now advertises `h2` and `http/1.1` in ALPN by default. `RustlsConfig::without_http2()` opts back out for HTTP/1.1-only deployments.

### Added
- `RustlsConfig::without_http2()` — drop `h2` from the advertised ALPN list
- `RustlsAcceptor::from_single_cert_no_h2(cert, key)` — convenience constructor for HTTP/1.1-only TLS, equivalent to `from_single_cert(cert, key).without_http2()`
- `RustlsClientTransport::negotiated_alpn()` / `RustlsServerTransport::negotiated_alpn()` — exposes the ALPN result for runtime/client dispatch

## [0.10.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0
- Trillium 1.0 uses [Swansong](https://docs.rs/swansong) instead of Stopper; `config().with_stopper(stopper)` becomes `config().with_swansong(swansong)`
- `RustlsConfig::spawn(fut)` → `RustlsConfig::runtime().spawn(fut)`

## [0.8.1](https://github.com/trillium-rs/trillium/compare/trillium-rustls-v0.8.0...trillium-rustls-v0.8.1) - 2024-06-29

### Fixed
- Require rustls-platform-verifier 0.3.2 to avoid multiple crypto backends

## [0.8.0](https://github.com/trillium-rs/trillium/compare/trillium-rustls-v0.7.0...trillium-rustls-v0.8.0) - 2024-04-12

### Added
- *(rustls)* [**breaking**] change how crypto providers are selected

See crate-level documentation for more on how the new features work. This is only a breaking change if you were using `default-features = false` AND not enabling either `ring` or `aws-lc-rs`. In that case you'll need to enable `custom-crypto-provider` on this crate, which brings in no additional dependencies but makes the possibility of a runtime panic due to crypto feature selection opt-in. Without this feature, misconfiguration (`default-features = false` without a crypto provider`) will be a compile-time error. 

### Other
- release

## [0.7.0](https://github.com/trillium-rs/trillium/compare/trillium-rustls-v0.6.0...trillium-rustls-v0.7.0) - 2024-04-03

### Added
- [**breaking**] upgrade rustls, use platform verifier

### Other
- *(actions)* tell cargo-udeps to ignore webpki-roots
- release

## [0.6.0](https://github.com/trillium-rs/trillium/compare/trillium-rustls-v0.5.0...trillium-rustls-v0.6.0) - 2024-01-24

### Other
- *(deps)* update env_logger requirement from 0.10.1 to 0.11.0
- Support using aws-lc-rs instead of ring
- Rename trillium-rustls `client` example so it doesn't conflict

## [0.5.0](https://github.com/trillium-rs/trillium/compare/trillium-rustls-v0.4.2...trillium-rustls-v0.5.0) - 2024-01-04

### Added
- *(rustls)* [**breaking**] add client and server features
- *(rustls)* [**breaking**] update trillium-rustls, switching to futures-rustls

## [0.4.2](https://github.com/trillium-rs/trillium/compare/trillium-rustls-v0.4.1...trillium-rustls-v0.4.2) - 2024-01-02

### Other
- updated the following local packages: trillium-server-common

## [0.4.1](https://github.com/trillium-rs/trillium/compare/trillium-rustls-v0.4.0...trillium-rustls-v0.4.1) - 2024-01-02

### Other
- upgrade deps
- remove dependency carats
- Make native root support optional
- deps
- fmt
