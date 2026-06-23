# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.2](https://github.com/trillium-rs/trillium/compare/trillium-openssl-v0.2.1...trillium-openssl-v0.2.2) - 2026-06-23

### Other

- updated the following local packages: trillium-server-common

## [0.2.1] - 2026-06-16

### Added

- The connector now implements `Connector::connect_to` (new in `trillium-server-common`): the
  pre-resolved addresses carried in the `Destination` are forwarded to the inner connector for the
  TCP dial, while the TLS server name still comes from the destination host — so address-pinned
  dialing works over TLS without affecting certificate validation.
- A non-empty per-connection ALPN list (`Destination::alpn`) sets the ALPN protocol list on that
  single connection, overriding the connector's configured default; an empty list (the default)
  leaves it in place.

## [0.2.0] - 2026-05-06

### Changed

- Compatible with trillium 1.1


## [0.1.0] - 2026-05-05

### Added

- Initial release: OpenSSL adapter for trillium.rs, providing `OpenSslAcceptor` for servers and
  `OpenSslConfig` for clients. Backed by [`async-openssl`](https://crates.io/crates/async-openssl)
  and [`openssl`](https://crates.io/crates/openssl); a third option alongside `trillium-rustls` and
  `trillium-native-tls`. Unlike `trillium-native-tls`, this crate negotiates ALPN, so HTTP/2 works
  on both client and server. Optional `vendored` feature forwards to `openssl/vendored` for a
  self-built OpenSSL.
