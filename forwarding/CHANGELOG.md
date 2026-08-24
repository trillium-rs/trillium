# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.1] - 2026-08-24

### Fixed
- `peer_ip` is now taken from the rightmost `forwarded-for` entry that isn't itself a trusted
  proxy, walking right to left, instead of the leftmost entry. Because the forwarded-for chain is
  append-only, everything to the left of the entry added by the outermost trusted proxy is
  attacker-controlled, so the previous behavior let any client behind a trusted proxy choose its
  own `peer_ip`.
- forwarded-for entries with ports (`192.0.2.60:8080`, `[2001:db8::17]:4711`) now parse.
- `proto` is compared case-insensitively, per RFC 7239.
- `Forwarding::trust_ips` includes the offending string in its panic message.

## [0.3.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Other
- release
- release
- clippy
- Release only rustls
- release
- release

## [0.2.3](https://github.com/trillium-rs/trillium/compare/trillium-forwarding-v0.2.2...trillium-forwarding-v0.2.3) - 2024-01-02

### Other
- updated the following local packages: trillium
