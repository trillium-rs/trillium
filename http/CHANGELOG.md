# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
