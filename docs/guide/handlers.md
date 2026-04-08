# A tour of handler libraries

Trillium is designed so that every feature is opt-in. The crates below are all official, all live in the same repository, and all follow the same `Handler` interface. Add the ones your application needs.

## Request handling

[**Router**](./handlers/router.md) — Pattern-based routing with named parameters (`:name`) and wildcards (`*`). Routers can be nested, so sub-applications can be mounted at a path. Any handler — including tuples and other routers — can be placed inside a route.

[**API Layer**](./handlers/api.md) — Extractor-based handlers for typed APIs. Write async functions that declare what they need from the request; the framework extracts it. Supports JSON bodies, form data, conn state, headers, and more. Return any type that implements `Handler`.

## Logging and observability

[**Logger**](./handlers/logger.md) — Structured HTTP request logging. Configurable format with composable formatters. Integrates with `trillium-conn-id` for per-request identifiers.

[**Conn ID**](./handlers/utilities.md#conn-id) — Assigns a unique identifier to each request, accessible on the conn and usable in log output.

## Auth

[**Basic Auth**](./handlers/utilities.md#basic-auth) — HTTP Basic Authentication. Returns `401 Unauthorized` with a `WWW-Authenticate` challenge for unauthenticated requests.

## Cookies and sessions

[**Cookies**](./handlers/cookies.md) — Parses inbound cookies and accumulates outbound `Set-Cookie` headers. Other handlers that need cookies (like sessions) must be placed after this one in the chain.

[**Sessions**](./handlers/sessions.md) — Server-side sessions keyed by a secure cookie. Pluggable session stores: in-memory and cookie stores are built in; database-backed stores are available via separate crates.

## Content serving

[**Static Files**](./handlers/static.md) — Serve files from disk with `trillium-static`, or embed them in the binary at compile time with `trillium-static-compiled`.

[**Template Engines**](./handlers/templates.md) — Integrations for Askama (compile-time, type-safe), Tera (runtime), and Handlebars (runtime).

[**Compression**](./handlers/compression.md) — Compresses response bodies with zstd, brotli, or gzip, selected based on the client's `Accept-Encoding` header.

[**Caching Headers**](./handlers/utilities.md#caching-headers) — Adds `ETag` and `Last-Modified` support, automatically returning `304 Not Modified` for unchanged resources.

## Real-time

[**Server-Sent Events**](./handlers/sse.md) — Persistent one-way connections for pushing events from server to browser. Simpler than WebSockets when bidirectional communication isn't needed.

[**WebSockets**](./handlers/websockets.md) — Full-duplex WebSocket connections. The WebSocket handler has access to the original request headers, path, and conn state, so auth and routing decisions made before the upgrade remain available.

[**Channels**](./handlers/channels.md) — Phoenix-style topic-based pub/sub over WebSocket. Clients join topics and receive messages scoped to those topics. Wire-compatible with the Phoenix JavaScript client.

[**WebTransport**](./handlers/webtransport.md) — Multiplexed streams and unreliable datagrams over HTTP/3 and QUIC. Suitable for applications that need lower latency or more transport control than WebSockets. Requires HTTP/3.

## HTTP client and proxying

[**HTTP Client**](./handlers/http_client.md) — A full-featured async HTTP client with connection pooling, TLS, and HTTP/3 support. Can also upgrade to WebSocket. Uses the same conn-based interface as the server.

[**Reverse Proxy**](./handlers/proxy.md) — Forward incoming requests to an upstream server and stream the response back.

## HTTP-level utilities

[**Head**](./handlers/utilities.md#head) — Handles `HEAD` requests correctly by running the handler chain and stripping the response body.

[**Method Override**](./handlers/utilities.md#method-override) — Rewrites the HTTP method based on a `_method` query parameter. Lets HTML forms submit `DELETE`, `PATCH`, and `PUT` requests.

[**Forwarding**](./handlers/utilities.md#forwarding) — Extracts the real remote IP and protocol from `Forwarded` and `X-Forwarded-*` headers when Trillium is behind a trusted reverse proxy.
