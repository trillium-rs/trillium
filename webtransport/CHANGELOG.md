# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0-rc.1](https://github.com/trillium-rs/trillium/releases/tag/trillium-webtransport-v0.1.0-rc.1) - 2026-04-08

### Added

- normalization pass
- [**breaking**] rename http_config to config
- *(trillium)* introduce façade types for Upgrade and RequestBody
- [**breaking**] add h3 support to client
- update how trillium-quinn handles crypto providers
- [**breaking**] webtransport and some tidying of h3/quic

### Other

- fix new crate versions
- release 1.0-rc.1 🌱
- *(deps)* upgrade async-tungstenite
- fix up broken docs links
- Add readmes
- clippy auto fix
- documentation pass

## [0.2.0] - 2026-04-08

### Added
- Initial release: WebTransport handler for HTTP/3 sessions
