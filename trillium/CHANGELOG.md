# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- `impl Handler` no longer requires `#[async_trait]` — remove the attribute from all Handler implementations; `async_trait` can likely be removed from your dependencies entirely
- `Box<dyn Handler>` is no longer valid for type-erased handlers; use `BoxedHandler::new(handler)` instead (see Added)
- `conn.headers()` / `conn.headers_mut()` disambiguated: use `conn.request_headers()` / `conn.request_headers_mut()` to read request headers, and `conn.response_headers()` / `conn.response_headers_mut()` for response headers
- `conn.with_header(name, val)` → `conn.with_response_header(name, val)` (builder) or `conn.insert_response_header(name, val)` (in-place)
- `conn.set_state(val)` → `conn.insert_state(val)` (was deprecated in 0.2.20)
- `conn.mut_state_or_insert_with(f)` → `conn.state_entry().or_insert_with(f)`
- `conn.stopper()` → `conn.swansong()` — trillium 1.0 uses [Swansong](https://docs.rs/swansong) instead of Stopper
- `conn.inner()` / `conn.inner_mut()` removed — methods previously accessed via `inner()` are now directly on `Conn`; if you find a method that's no longer accessible this way, please open an issue
- `Info` no longer implements `Clone` — it now wraps a shared `Arc`-backed `TypeSet` (see Added)
- `Info::listener_description()` and `Info::server_description()` removed — store custom descriptions in shared state if needed
- `Init` has completely new semantics: `Init<T: Handler>` (a handler wrapper) is replaced by `Init<F: FnOnce(Info) -> Future<Output = Info>>` (a closure-based startup initializer) — see Added
- `Transport` moved from `trillium_http::transport::Transport` to `trillium::Transport`
- `trillium::Conn::request_body` now returns a `trillium::request_body::RequestBody` instead of a `trillium_http::ReceivedBody`
- `trillium::Upgrade` is no longer a reexport of `trillium_http::Upgrade` and has a new interface.

### Added

#### Shared server-level state

`Info` now wraps a shared `TypeSet` (backed by `ServerConfig`) that persists for the lifetime of the server. Handlers insert values into this set during `Handler::init` or by placing an `Init` handler first in the tuple:

```rust
Init::new(|mut info| async move {
    let db = MyDb::connect("db://...").await.unwrap();
    info.with_state(db)
})
```

Any handler downstream can then read that value via `conn.shared_state::<T>()`:

```rust
|conn: Conn| async move {
    let db = conn.shared_state::<MyDb>().unwrap();
    conn.ok(db.query("select ...").await)
}
```

Because `Info` now wraps a shared `Arc`, it can no longer implement `Clone`. The `Init` closure takes `Info` by value and returns it, allowing async setup before connections are accepted.

#### Server lifecycle: `ServerHandle` and `BoundInfo`

`Config::spawn(handler)` now returns a `ServerHandle` that is cheaply `Clone` and covers the full server lifecycle:

- `await server_handle` — waits for the server to shut down (like joining a thread)
- `server_handle.info().await` — waits for the server to finish binding and initialization, then returns a `BoundInfo` with the bound address, URL, and any values inserted into shared state during `Handler::init`
- `server_handle.shut_down()` — initiates graceful shutdown and returns a `ShutdownCompletion` that can be awaited
- `server_handle.swansong()` — retrieves a `Swansong` for coordinating shutdown manually

`BoundInfo` provides an immutable snapshot of the server's shared state after initialization: `bound_info.tcp_socket_addr()`, `bound_info.url()`, `bound_info.state::<T>()`.

This makes it straightforward to spawn a server in the background and then wait for it to be ready before sending requests — useful for integration tests and for programs that need to know the bound port when using `PORT=0`.

#### Other additions

- `BoxedHandler` — type-erased `Handler` with `downcast`, `downcast_ref`, `downcast_mut`, and `is` for downcasting; replaces `Box<dyn Handler>`
- `conn.shared_state::<T>()` — read server-level shared state
- `conn.state_entry::<T>()` — entry API for per-connection state, mirrors `HashMap::entry`
- `conn.swansong()` — access the [Swansong](https://docs.rs/swansong) graceful shutdown handle
- `conn.host()`, `conn.path_and_query()`, `conn.http_version()` — new convenience accessors
- `Transport` trait is now public at `trillium::Transport` — implement it for custom transports (QUIC, WebTransport, etc.)
- `set_*` setters on `Conn` (e.g. `set_status`, `set_body`) now return `&mut Self`, enabling chaining: `conn.set_status(200).set_body("ok")`

### HTTP/3

Trillium 1.0 introduces HTTP/3 support via the new [`trillium-quinn`](https://docs.rs/trillium-quinn) crate. No changes to existing handlers are required — HTTP/3 connections run the same `Handler` pipeline as HTTP/1.1. Enable alongside your existing server by adding a QUIC config:

```rust
trillium_tokio::config()
    .with_acceptor(RustlsAcceptor::from_single_cert(&cert_pem, &key_pem))
    .with_quic(trillium_quinn::QuicConfig::from_single_cert(&cert_pem, &key_pem))
    .run(handler);
```

## [0.2.20](https://github.com/trillium-rs/trillium/compare/trillium-v0.2.19...trillium-v0.2.20) - 2024-05-30

### Added
- deprecate set_state for insert_state

## [0.2.19](https://github.com/trillium-rs/trillium/compare/trillium-v0.2.18...trillium-v0.2.19) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Fixed
- *(trillium)* fix the flaky liveness test

## [0.2.18](https://github.com/trillium-rs/trillium/compare/trillium-v0.2.17...trillium-v0.2.18) - 2024-04-04

### Fixed
- *(trillium)* move futures-lite to dev-dependencies

## [0.2.17](https://github.com/trillium-rs/trillium/compare/trillium-v0.2.16...trillium-v0.2.17) - 2024-03-22

### Added
- *(trillium)* improve log message when calling `Arc<Handler>::init` on a clone

### Other
- clippy

## [0.2.16](https://github.com/trillium-rs/trillium/compare/trillium-v0.2.15...trillium-v0.2.16) - 2024-02-09

### Fixed
- *(trillium)* downgrade Arc<Handler>::init from a panic to a warning
- *(testing)* TestTransport behaves like TcpStream regarding closure

### Other
- *(trillium)* add test for is_disconnected

## [0.2.15](https://github.com/trillium-rs/trillium/compare/trillium-v0.2.14...trillium-v0.2.15) - 2024-02-05

### Fixed
- *(trillium)* fix trillium-http dependency

## [0.2.14](https://github.com/trillium-rs/trillium/compare/trillium-v0.2.13...trillium-v0.2.14) - 2024-02-05

### Added
- *(trillium)* reexpose trillium-http features
- *(http)* cancel on disconnect

### Other
- *(trillium)* add liveness (cancel-on-disconnect) test

## [0.2.13](https://github.com/trillium-rs/trillium/compare/trillium-v0.2.12...trillium-v0.2.13) - 2024-01-02

### Other
- updated the following local packages: trillium-http

## [0.2.12](https://github.com/trillium-rs/trillium/compare/trillium-v0.2.11...trillium-v0.2.12) - 2024-01-02

### Other
- update dependencies other than trillium-rustls
- use trillium-http@v0.3.8
- use trillium-http@v0.3.7
- deps
- 📎💬
- bump trillium-http
- upgrade deps
- Expose `start_time()` on `trillium::Conn` to avoid needing `inner()`
