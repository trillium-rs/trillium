# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.3](https://github.com/trillium-rs/trillium/compare/trillium-proxy-v0.5.2...trillium-proxy-v0.5.3) - 2024-01-04

### Other
- updated the following local packages: trillium-server-common, trillium-client, trillium-forwarding

## [0.5.2](https://github.com/trillium-rs/trillium/compare/trillium-proxy-v0.5.1...trillium-proxy-v0.5.2) - 2024-01-02

### Other
- updated the following local packages: trillium-http, trillium-server-common, trillium-client

## [0.5.1](https://github.com/trillium-rs/trillium/compare/trillium-proxy-v0.5.0...trillium-proxy-v0.5.1) - 2024-01-02

### Other
- whatever clippy wants, clippy gets
- use pooling in example
- proxy patch: only stream the request body if it's present, and handle get requests with bodies
- proxy patch bugfix: forward proxy connect needs to halt
