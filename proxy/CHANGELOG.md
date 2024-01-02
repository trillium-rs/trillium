# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.1](https://github.com/trillium-rs/trillium/compare/trillium-proxy-v0.5.0...trillium-proxy-v0.5.1) - 2024-01-02

### Other
- whatever clippy wants, clippy gets
- use pooling in example
- proxy patch: only stream the request body if it's present, and handle get requests with bodies
- proxy patch bugfix: forward proxy connect needs to halt
