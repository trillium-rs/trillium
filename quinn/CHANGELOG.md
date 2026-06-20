# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `QuicConfig::with_transport_config` — override the quinn `TransportConfig` (flow-control windows,
  send fairness, congestion control, GSO) on a constructed config, composing with the `from_*`
  constructors.
- `ClientQuicConfig::with_transport_config` — the client-side mirror of the above, for tuning the
  quinn `TransportConfig` on outbound HTTP/3 connections.

## [0.1.6] - 2026-06-18

### Added

- `QuinnTransport` and `QuinnSend` implement `QuicTransportSend::set_priority`, forwarding to
  quinn's per-stream send priority so HTTP/3 response prioritization takes effect.

## [0.1.5] - 2026-06-16

### Added

- `QuinnEndpoint::connect_with_alpn` (implementing the new `trillium-server-common` seam) — initiate
  a QUIC connection advertising a per-connection ALPN list, distinct from the endpoint's default, so
  one endpoint can serve both `h3` and `doq` over a single UDP socket. Available when the
  `ClientQuicConfig` was built from a rustls config; unavailable on configs built via
  `from_quinn_client_config`.

## [0.1.4] - 2026-05-31

### Added

- `QuinnConnection`, `QuinnTransport`, `QuinnSend`, `QuinnRecv` — re-exported at the crate root.
- `QuinnEndpoint::local_addr` — returns the address the underlying `quinn::Endpoint` is bound to.

### Changed

- `QuicConfig::bind_with_socket` is overridden to consume a pre-claimed `std::net::UdpSocket` directly, used by the new `trillium-server-common` `ServerBuilder::bind_quic` path. The existing `QuicConfig::bind(addr, …)` path is unchanged; it now binds the socket and delegates to `bind_with_socket`.

## [0.1.3] - 2026-05-11

### Added
- `QuicConfig::from_cert_resolver` — build a `QuicConfig` from an `Arc<dyn rustls::server::ResolvesServerCert>`. Lets callers supply a dynamic certificate source (e.g. an ACME integration) without rebuilding the QUIC server config on rotation; if the resolver returns `None`, the TLS handshake fails and the connection is rejected, so binding before the first cert is available is safe.

## [0.1.2] - 2026-05-05

### Fixed
- Bump `trillium-server-common` dependency specifier to `0.7` to match the 1.1 release; `0.1.1` was published with a stale `0.6` spec.

## [0.1.1] - 2026-05-05 [YANKED]

### Added
- `pub use quinn` — the underlying `quinn` crate is now re-exported at the crate root, so callers don't need to add `quinn` as a separate dependency to interact with the underlying QUIC types

## [0.1.0] - 2026-05-02

### Added
- Initial release: Quinn-backed QUIC adapter for HTTP/3 support
