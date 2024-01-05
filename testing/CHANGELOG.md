# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.3](https://github.com/trillium-rs/trillium/compare/trillium-testing-v0.5.2...trillium-testing-v0.5.3) - 2024-01-05

### Other
- updated the following local packages: trillium-server-common

## [0.5.2](https://github.com/trillium-rs/trillium/compare/trillium-testing-v0.5.1...trillium-testing-v0.5.2) - 2024-01-02

### Added
- *(testing)* allow test(harness = trillium_testing::harness) to return ()

### Other
- use #[test(harness)] instead of #[test(harness = harness)]
- Update test-harness requirement from 0.1.1 to 0.2.0

## [0.5.1](https://github.com/trillium-rs/trillium/compare/trillium-testing-v0.5.0...trillium-testing-v0.5.1) - 2024-01-02

### Fixed
- fix runtimeless test

### Other
- use trillium-http@v0.3.8
- use trillium-http@v0.3.7
- deps
- 📎💬
- bump trillium-http
- upgrade deps
- testing breaking: spawn returns a runtime agnostic join handle
- remove dependency carats
- Update futures-lite requirement from 1.13.0 to 2.0.0
- deps
- clippy fixes
- clippy doesn't like big types
- testing patch feature: add support for running tests without a runtime
- clipped
- use Config::spawn to implement with_server, expose config and client config
- actually fix dns in test mode
