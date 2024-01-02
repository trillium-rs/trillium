# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.10](https://github.com/trillium-rs/trillium/compare/trillium-http-v0.3.9...trillium-http-v0.3.10) - 2024-01-02

### Other
- Update smol requirement from 1.3.0 to 2.0.0
- update dependencies other than trillium-rustls
- http patch reversion: set Server header before request again
- http patch feature: serde for version
- http patch: don't send explicit connection: keep-alive
