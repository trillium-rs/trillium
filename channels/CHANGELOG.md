# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.0] - 2026-11-19

### Changed
- **Breaking**: depends on `trillium-websockets` 0.9 (was 0.7), so `ChannelConn` dereferences to that version's `WebSocketConn`.
- **Breaking**: browsers can only connect from a page on the same host as the socket. An application whose pages and sockets are on different hosts must name the page origins with `Channel::allow_origins`.

### Added
- `ChannelConn::reject` and `ChannelClient::reject` send a correlated error reply for a join or push, without subscribing the client to the topic.
- `Channel::allow_origins`, `Channel::allow_origin_fn`, and `Channel::allow_any_origin` configure which pages may connect.
- `Channel::with_protocol_config` configures websocket limits such as maximum message and frame sizes, and `WebSocketConfig` is reexported.
- `ChannelConn::mark_channel_errored` and `ChannelClient::mark_channel_errored` send a `phx_error`, which the Phoenix JavaScript client treats as a crashed channel and rejoins.

### Deprecated
- `ChannelConn::reply_error` and `ChannelClient::reply_error`, which send a `phx_error` rather than an error reply. To refuse a join or push, use `reject`; to put the client's channel in an error state, use `mark_channel_errored`.

### Fixed
- WebSocket upgrades honor the `vsn` query parameter, so Phoenix V2 clients can join and push events.
- Phoenix JavaScript clients discarded broadcasts that originated from another client's push. Broadcasts no longer carry the originating `ref` or `join_ref`, so they reach every subscriber.
- Clients receive the `allow_leave` acknowledgment after unsubscribing from the topic.

## [0.4.0] - 2026-05-02

### Changed
- Compatible with trillium 1.0
- `ChannelHandler` no longer uses `#[async_trait]`; remove the attribute from any implementations in your code

### Added
- deprecate set_state for insert_state

### Other
- release
- release
- release
- *(deps)* update async-broadcast requirement from 0.6.0 to 0.7.0
- Release only rustls
- release
- release

## [0.3.1](https://github.com/trillium-rs/trillium/compare/trillium-channels-v0.3.0...trillium-channels-v0.3.1) - 2024-01-02

### Other
- update dependencies other than trillium-rustls
- (cargo-release) version 0.6.0
- deps
- upgrade deps
- remove dependency carats
- Update futures-lite requirement from 1.13.0 to 2.0.0
- Update async-broadcast requirement from 0.5.1 to 0.6.0
- deps
