# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
