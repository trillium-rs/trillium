# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0-rc.1](https://github.com/trillium-rs/trillium/releases/tag/trillium-quinn-v0.1.0-rc.1) - 2026-04-08

### Added

- http trailers
- [**breaking**] add h3 support to client
- update how trillium-quinn handles crypto providers
- update how quinn handles crypto providers
- [**breaking**] webtransport and some tidying of h3/quic
- auto-populate alt-svc response header
- [**breaking**] hypertext transfer protocol, three

### Fixed

- client now is appropriately factored, uses H3Connection

### Other

- fix new crate versions
- release 1.0-rc.1 🌱
- *(deps)* upgrade async-tungstenite
- Add readmes
- some manual clippy fixes
- clippy auto fix
- *(deps)* [**breaking**] update all deps
- fmt
- tidy up the h3 example

## [0.2.0] - 2026-04-08

### Added
- Initial release: Quinn-backed QUIC adapter for HTTP/3 support
