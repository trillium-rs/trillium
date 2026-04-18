# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.8.0-rc.2](https://github.com/trillium-rs/trillium/compare/trillium-testing-v0.8.0-rc.1...trillium-testing-v0.8.0-rc.2) - 2026-04-18

### Other

- updated the following local packages: trillium-http, trillium-server-common, trillium-client, trillium, trillium-smol, trillium-async-std, trillium-tokio

## [0.8.0] - 2026-04-08

### Changed
- Compatible with trillium 1.0
- `init(&mut handler)` is now async and returns `Arc<HttpContext>`: `init(&mut handler).await`; capture the returned value if you need to pass it to `TestConn::with_context()`
- `ClientConfig` struct removed; use the `client_config()` function or `RuntimelessClientConfig` directly
- `SpawnHandle<F>` removed; background task handles are now `DroppableFuture` from `trillium-server-common`

### Added
- Introduce new testing approach described at `TestHandler`:

```rust
use trillium::{Conn, Status, conn_try};
use trillium_testing::TestServer;

async fn handler(mut conn: Conn) -> Conn {
    let Ok(request_body) = conn.request_body_string().await else {
        return conn.with_status(500).halt();
    };

    conn.with_body(format!("request body was: {}", request_body))
        .with_status(418)
        .with_response_header("request-id", "special-request")
}

let app = TestServer::new(handler).await;
app.post("/")
    .with_body("hello trillium!")
    .await
    .assert_status(Status::ImATeapot)
    .assert_body("request body was: hello trillium!")
    .assert_headers([
        ("request-id", "special-request"),
        ("content-length", "33")
    ]);

```

- The assertion macros (`assert_ok!`, `assert_status!`, `assert_not_handled!`, etc.) and request builders are unchanged
- Zero-dependency testing: when no runtime feature is enabled, `RuntimelessRuntime`, `RuntimelessServer`, and `RuntimelessClientConfig` provide fully in-memory test infrastructure without requiring tokio, smol, or async-std
- `with_runtime(|runtime| async { ... })` — test harness that injects a `Runtime` into the test closure, also usable as a test harness
- `TestConn::with_context(Arc<HttpContext>)` — pass a server config (including shared state initialized by `init`) to a test connection

## [0.8.0-rc.1](https://github.com/trillium-rs/trillium/compare/trillium-testing-v0.6.1...trillium-testing-v0.8.0-rc.1) - 2024-05-30

### Added
- *(api)* [**breaking**] make IoErrors respond with BadRequest
- deprecate set_state for insert_state

### Other
- release

## [0.6.1](https://github.com/trillium-rs/trillium/compare/trillium-testing-v0.6.0...trillium-testing-v0.6.1) - 2024-04-07

### Added
- add deprecation warnings to 0.2 branch in preparation for 1.0

### Fixed
- *(trillium)* fix the flaky liveness test

### Other
- release
- release
- release
- release

## [0.6.0](https://github.com/trillium-rs/trillium/compare/trillium-testing-v0.5.4...trillium-testing-v0.6.0) - 2024-03-22

### Fixed
- *(testing)* [**breaking**] RuntimelessClientConfig must be constructed with default or new

### Other
- clippy

## [0.5.4](https://github.com/trillium-rs/trillium/compare/trillium-testing-v0.5.3...trillium-testing-v0.5.4) - 2024-02-08

### Added
- *(testing)* runtimeless testing randomizes port zero

### Fixed
- *(testing)* TestTransport behaves like TcpStream regarding closure

### Other
- *(testing)* add tests for cancel-on-disconnect using synthetic conns

## [0.5.3](https://github.com/trillium-rs/trillium/compare/trillium-testing-v0.5.2...trillium-testing-v0.5.3) - 2024-02-05

### Added
- *(testing)* reexport some server-common traits

### Fixed
- *(testing)* use host:port for runtimeless info for consistency with runtime adapters
- *(testing)* TestTransport closure is symmetrical

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
