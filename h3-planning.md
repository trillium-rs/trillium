# H3/H2 Planning Notes

## server-config Branch Changes (vs main)

The `server-config` branch (based on `0.3.x`) has significant structural improvements relevant to H3:

### Key changes:
1. **`ServerConfig` introduced** — Arc-shared struct holding `HttpConfig`, `Swansong`, and shared `TypeSet`. Replaces scattered config passing. The H1 connection loop now lives on `ServerConfig::run()` (`Arc<Self>` method).
2. **`Stopper` → `Swansong`** — graceful shutdown now uses swansong crate, with guard-based connection counting.
3. **`StateSet` → `TypeSet`** — extracted to `type_set` crate, includes `Entry` API.
4. **H1 implementation split** — Protocol-specific methods moved from `conn.rs` to `conn/implementation.rs`. The main `conn.rs` now only has protocol-agnostic accessors.
5. **`Runtime` trait** — `Server` now has `type Runtime: RuntimeTrait`. Runtime is pluggable.
6. **Custom parser** (`parse` feature) — Trillium has its own HTTP/1.x parser (no httparse), parsing directly into trillium Headers. `Headers::parse()`, `Method::parse()`, `Version::parse()` exist.
7. **`async_trait` eliminated** — Uses RPITIT instead.
8. **`RunningConfig`** — server-common split: `Config` (builder) → `RunningConfig` (runtime state with `Arc<ServerConfig>`).

### What this means for H3:
- `ServerConfig` is already the right shape for shared server state across protocols.
- The `conn/implementation.rs` split is a step toward what we need — the protocol-agnostic Conn fields vs H1-specific logic are already partially separated.
- `RunningConfig::handle_stream` calls `self.server_config.clone().run(transport, handler)` — this is the H1 entry point. H3 would need a parallel `run_h3()` or similar.
- The custom parser work suggests appetite for owning the full parsing stack, which aligns with building QPACK.

## Architecture Analysis (server-config branch)

### trillium_http::Conn<Transport> fields
Protocol-agnostic: server_config, request_headers, response_headers, path, method, status, version, state, response_body, secure, after_send, start_time, peer_ip.
H1-specific: transport, buffer, request_body_state.

### Protocol-coupled code (all in conn/implementation.rs):
1. `new_internal` — H1 request parsing (httparse or custom parser)
2. `send` — H1 response writing (status line + CRLF headers + body via copy)
3. `write_headers` — H1 wire format serialization
4. `head` — Reading bytes until `\r\n\r\n`
5. `next` — Drains body then parses next request on same transport (keepalive)
6. `should_close`/`should_upgrade`/`finish` — H1 connection lifecycle
7. `validate_headers` — H1-specific (content-length vs transfer-encoding conflict)
8. `build_request_body` — Creates ReceivedBody with chunked/fixed-length state
9. `send_100_continue` / `needs_100_continue` — H1.1-specific

### Protocol-coupled code still in conn.rs:
1. `finalize_headers` — Adds Date, Content-Length, Transfer-Encoding, Connection headers (H1-specific logic)
2. `request_body()` — Calls needs_100_continue/send_100_continue (H1-specific)

### Body (response) — still has chunked encoding in AsyncRead impl
### ReceivedBody — still handles chunked decoding/fixed-length

## Key Separation Boundaries

### What needs to be protocol-specific:
1. **Request parsing**: H1 text parsing vs QPACK-decoded HEADERS frames
2. **Response serialization**: Status line + CRLF headers vs QPACK-encoded HEADERS frames
3. **Body framing**: chunked/content-length vs DATA frames
4. **Connection lifecycle**: keepalive loop vs multiplexed streams
5. **Flow control**: H3 has stream-level and connection-level
6. **Server push**: H3 has PUSH_PROMISE
7. **Header finalization**: Different headers are auto-generated per protocol

### What should be shared:
1. `Headers` type (wire format differs but the map is protocol-agnostic)
2. `Method`, `Status`, `Version` enums
3. `Body` type (but chunked encoding must be extracted)
4. `TypeSet`, `ServerConfig` (minus HttpConfig's H1-specific fields)
5. Handler trait + `trillium::Conn` (handler-facing API)
6. `after_send` hooks, `start_time`, `peer_ip`, `secure`

### KnownHeaderName gaps
- No pseudo-headers (`:method`, `:path`, `:scheme`, `:authority`, `:status`) — needed for H2/H3
- Contains H1-only headers (`Connection`, `Transfer-Encoding`, `Keep-Alive`, `Upgrade`) that are forbidden in H3

## QUIC Library Comparison

### quinn (0.11.9) — High-level async QUIC
- **Architecture**: quinn-proto (pure logic, no I/O) + quinn (async runtime integration)
- **Runtime abstraction**: `quinn::Runtime` trait with `new_timer`, `spawn`, `wrap_udp_socket`. Built-in impls: `TokioRuntime`, `SmolRuntime`, `AsyncStdRuntime`.
- **Socket abstraction**: `AsyncUdpSocket` trait — poll-based UDP send/recv. Runtime-specific.
- **Endpoint**: `Endpoint::new` takes `Arc<dyn Runtime>`. `Endpoint::new_with_abstract_socket` takes `Arc<dyn AsyncUdpSocket>` for full injectable control.
- **Connection**: `accept_bidi()` → `(SendStream, RecvStream)`, `accept_uni()`, `open_bi()`, `open_uni()`. Also: `remote_address()`, `close()`, `stats()`, etc.
- **Streams**: `SendStream` impls `AsyncWrite`. `RecvStream` impls `AsyncRead`. Both futures-lite compatible. Can serve as trillium transports directly.
- **No H3**: quinn is QUIC-only. H3 framing (HEADERS/DATA frames, QPACK) is trillium's job.

### s2n-quic (1.75.0) — AWS's QUIC implementation
- **Architecture**: Provider-based. Pluggable `provider::io`, `provider::tls`, `provider::congestion_controller`, etc.
- **Runtime**: **Tokio-only** for IO (`provider::io::tokio` is the only IO provider).
- **Connection**: `accept_bididirectional_stream()`, `accept_receive_stream()`, `open_bidirectional_stream()`, `open_send_stream()`. Also: `split()` into `(Handle, StreamAcceptor)`.
- **Streams**: `BidirectionalStream` impls BOTH `futures::AsyncRead`/`AsyncWrite` AND `tokio::AsyncRead`/`AsyncWrite`. Supports `split()` into `(ReceiveStream, SendStream)`.
- **Server**: `Server::accept()` returns `Connection`. Builder pattern with providers.
- **No H3**: Like quinn, QUIC-only. No built-in HTTP/3.
- **Key limitation**: Tokio-only IO means it can't back smol/async-std trillium adapters.

### quiche (0.26.0) — Cloudflare's low-level QUIC + HTTP/3
- **Architecture**: Fully synchronous/poll-based. Zero async — app provides all I/O and timers.
- **QUIC API**: `conn.recv(&mut buf)` / `conn.send(&mut buf)` — app manages socket reads/writes.
- **Streams**: `conn.stream_send(stream_id, data, fin)` / `conn.stream_recv(stream_id, buf)`. No stream objects, just IDs.
- **Includes H3**: `quiche::h3::Connection` provides HTTP/3 frame handling. Event-driven: `h3_conn.poll(&mut conn)` returns `Event::Headers`, `Event::Data`, `Event::Finished`, etc.
- **Includes QPACK**: Built into the H3 layer. Uses pseudo-headers (`:method`, `:path`, `:status`).
- **Runtime-independent by being IO-independent**: No async at all. Would need a full async adapter layer.
- **Verdict**: Too low-level for trillium's architecture. Would require building essentially what quinn provides.

### Summary table

| | **quinn** | **s2n-quic** | **quiche** |
|---|---|---|---|
| Async model | Async-native | Async-native | Sync (app manages I/O) |
| Runtime | Pluggable (tokio/smol/async-std) | **Tokio-only** | N/A |
| Streams | AsyncRead/AsyncWrite | AsyncRead/AsyncWrite (both ecosystems) | Raw send/recv by stream ID |
| H3 | No (separate concern) | No | Yes (built-in + QPACK) |
| Viability for trillium | **Best fit** — runtime pluggable, stream types compatible | Usable but tokio-only | Too much glue needed |

## Design Decisions (Agreed)

### 1. finalize_headers — version-aware dispatch
- Switch on `self.version` in `finalize_headers`
- H1: existing logic (Transfer-Encoding, Connection, Content-Length framing)
- H2/H3: strip forbidden headers (Connection, Transfer-Encoding, Keep-Alive, Upgrade), Content-Length is advisory only
- Also strip forbidden headers from incoming H3 request headers during construction
- Could auto-insert `Alt-Svc` header on H1 responses when H3 is configured

### 2. Body/ReceivedBody — protocol-aware state machines
- Extend existing state machines rather than extracting chunked encoding
- Body: switch on version to produce chunked (H1) or raw/DATA-framed (H3) output
- ReceivedBody: add DATA frame state alongside Chunked/FixedLength states
- DATA frame decoding is simpler than chunked (varint length + payload)
- **Watch out**: H3 stream interleaving — HEADERS/DATA/HEADERS(trailers) on same stream. ReceivedBody may need frame-type awareness to know when DATA ends.

### 3. Abstraction layers — trillium owns the interfaces

**Principle**: A new runtime adapter (trillium-newruntime) should be the only thing needed for full H3 support. No quinn-specific or s2n-quic-specific types in public trillium interfaces.

**Server trait owns network IO** (including UDP for QUIC). Runtime handles non-networking async (spawn, timers, signals). This mirrors the existing split: Server already owns TCP accept, RuntimeTrait owns spawn/delay/interval.

### 4. Trillium QUIC traits

```
trait QuicConnection: Send + Sync + 'static {
    type BiStream: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static;
    type SendStream: AsyncWrite + Send + Sync + Unpin + 'static;
    type RecvStream: AsyncRead + Send + Sync + Unpin + 'static;

    fn accept_bidi(&mut self) -> impl Future<Output = Option<Self::BiStream>> + Send;
    fn accept_uni(&mut self) -> impl Future<Output = Option<Self::RecvStream>> + Send;
    fn open_uni(&self) -> impl Future<Output = io::Result<Self::SendStream>> + Send;
    fn remote_addr(&self) -> io::Result<SocketAddr>;
}

trait QuicEndpoint: Send + Sync + 'static {
    type Connection: QuicConnection;

    fn accept(&self) -> impl Future<Output = Option<Self::Connection>> + Send;
    fn local_addr(&self) -> io::Result<SocketAddr>;
    fn close(&self);
}
```

### 5. Server trait extension for QUIC

Rather than putting UDP on RuntimeTrait, UDP binding is a **Server** concern. Options:
- Extend `Server` trait with optional QUIC methods (default impls that do nothing)
- Separate `QuicCapable` trait that `Server` impls can additionally implement
- Server-level config that accepts a QUIC provider

The Server already owns TCP listener setup (`from_tcp`, `from_unix`, `accept`). UDP/QUIC fits naturally alongside.

### 6. QUIC implementation adapters (separate crates)

- **trillium-quinn**: Implements `QuicEndpoint`/`QuicConnection` by bridging to quinn. Adapts trillium's UDP/runtime interfaces to quinn's `Runtime` + `AsyncUdpSocket`. Available for all trillium runtimes.
- **trillium-s2n-quic**: Same for s2n-quic. Limited to trillium-tokio since s2n-quic is tokio-only.
- Having both validates the abstraction — if the traits work for both, they'll work for others.

### 7. Config-level H3 opt-in

```
trillium_tokio::config()
    .with_acceptor(rustls_acceptor)
    .with_h3(trillium_quinn::QuicConfig::new(tls_config))
    .run(handler);
```

- `with_h3` accepts anything implementing a provider trait
- Server serves H1 over TCP+TLS and H3 over QUIC simultaneously
- H3 availability advertised via `Alt-Svc` header on H1 responses
- Same handler serves both protocols — `trillium::Conn` is unchanged

## Trillium RuntimeTrait (for reference)

```rust
trait RuntimeTrait: Into<Runtime> + Clone + Send + Sync + 'static {
    fn spawn<Fut>(&self, fut: Fut) -> DroppableFuture<...>;
    fn delay(&self, duration: Duration) -> impl Future<Output = ()> + Send;
    fn interval(&self, period: Duration) -> impl Stream<Item = ()> + Send + 'static;
    fn block_on<Fut>(&self, fut: Fut) -> Fut::Output;
    fn timeout(&self, duration: Duration, fut: Fut) -> impl Future<Output = Option<...>>;
    fn hook_signals(&self, signals: impl IntoIterator<Item = i32>) -> impl Stream<...>;
}
```

quinn::Runtime needs: `spawn`, `new_timer`, `wrap_udp_socket`
- `spawn` and `new_timer` can be shimmed from RuntimeTrait's `spawn` and `delay`
- `wrap_udp_socket` maps to the Server-level UDP binding concern

## Decision: H3 only (no H2 for now)

H2 would require trillium to own a full protocol implementation (framing, HPACK, flow control, stream state machine) — significant maintenance burden. H3 is the bigger win for users: most browsers upgrade directly from H1 to H3 via `Alt-Svc`. H2 can be added later if needed.

Key difference in scope: H3 delegates multiplexing, flow control, congestion control, and encryption to the QUIC layer (quinn/s2n-quic). Trillium only owns H3 framing and QPACK. H2 would require trillium to own all of that itself over TCP.

If H2 is added later, shared code with H3 would be limited to Huffman coding and variable-length integer encoding. HPACK and QPACK dynamic tables differ substantially (QPACK has out-of-order delivery coordination via encoder/decoder instruction streams).

## QPACK Architecture

### State ownership
- **Dynamic table state is per-QUIC-connection**, not per-request. One encoder table and one decoder table shared across all streams (requests) on a connection.
- **Encoder/decoder sync** happens over dedicated unidirectional streams on each connection.
- **`Headers` stays protocol-agnostic** — it's just data going in/out of the codec. No `Headers::format_as_h3()`.

### Codec API shape
```rust
struct QpackEncoder { /* dynamic table, max size, etc. */ }
struct QpackDecoder { /* dynamic table, max size, etc. */ }

impl QpackEncoder {
    fn encode(&mut self, headers: &Headers, buf: &mut Vec<u8>);
}
impl QpackDecoder {
    fn decode(&mut self, buf: &[u8]) -> Result<Headers, QpackError>;
}
```

The `&mut self` signatures accommodate future dynamic table state even when starting with static-table-only. The codec structs live on the H3 connection handler (alongside `ServerConfig`), not on `Headers` or `Conn`.

### QUIC connection vs stream hierarchy
- **QUIC connection** (long-lived) — one per client. Owns QPACK dynamic tables, H3 SETTINGS, control/encoder/decoder unidirectional streams.
- **QUIC stream** (short-lived) — one bidirectional stream per request/response. Carries H3 frames: HEADERS → DATA → optional trailing HEADERS.
- Analogous to H1: the QUIC connection is like the keepalive loop (`next()`), each stream is like one request/response cycle. But concurrent instead of sequential.

## Implementation Sequence

Top-down: start with the unit-testable protocol code, work outward to network I/O.

### Phase 1: Shared primitives (in trillium-http, feature-gated)
1. **Huffman codec** — fixed table (shared with any future HPACK). Encode/decode bytes. Well-specified, ~200-300 lines.
2. **Variable-length integer encoding** — prefix-coded varints per RFC 7541 §5.1 / RFC 9204 §4.1.1. Shared with any future HPACK. Small (~50-100 lines).

### Phase 2: QPACK (in trillium-http)
3. **QPACK static table** — 99 entries, hardcoded lookup.
4. **QPACK decoder** — decode encoded header blocks into `Headers`. Static-table-only first, `&mut self` signature ready for dynamic table.
5. **QPACK encoder** — encode `Headers` into header blocks. Static-table-only first.
6. **Test against RFC 9204 test vectors.**

### Phase 3: H3 framing (in trillium-http)
7. **H3 frame parser/serializer** — varint type + varint length + payload. Frame types: DATA, HEADERS, SETTINGS, GOAWAY, CANCEL_PUSH, PUSH_PROMISE. Testable against in-memory `AsyncRead`/`AsyncWrite`.
8. **H3 SETTINGS handling** — parse/serialize settings frames, store as connection state.

### Phase 4: Conn integration (in trillium-http)
9. **`finalize_headers` version dispatch** — H3 path strips forbidden headers, Content-Length advisory only.
10. **`ReceivedBody` DATA frame state** — add H3 body state alongside Chunked/FixedLength. Frame-type awareness for HEADERS/DATA/trailers boundaries on a stream.
11. **Response `Body`** — H3 path writes DATA frames instead of chunked encoding.
12. **Pseudo-header handling** — `:method`, `:path`, `:scheme`, `:authority` extracted during H3 request construction; `:status` emitted during response.

### Phase 5: QUIC traits (in trillium-server-common or trillium-http)
13. **`QuicConnection` / `QuicEndpoint` traits** — as sketched in design decisions §4.
14. **H3 connection handler** — accepts streams from `QuicConnection`, runs QPACK codec + H3 framing, constructs `Conn`, invokes handler. Manages control/encoder/decoder unidirectional streams.

### Phase 6: quinn adapter (new crate: trillium-quinn)
15. **Implement `QuicEndpoint`/`QuicConnection`** for quinn types.
16. **Shim trillium's `RuntimeTrait`** to quinn's `Runtime` trait (`spawn` → `spawn`, `delay` → `new_timer`).
17. **UDP socket binding** — Server-level concern, bridged to quinn's `AsyncUdpSocket`.

### Phase 7: Server wiring
18. **`with_h3()` config** — accepts QUIC provider, starts UDP listener alongside TCP.
19. **`Alt-Svc` header** — auto-insert on H1 responses when H3 is configured.
20. **Graceful shutdown** — Swansong integration for H3 connections (GOAWAY + drain).

## Decision: Server Push via Conn state

Server push (PUSH_PROMISE) is strictly cache prewarming — the server sends a synthetic request/response pair for a resource the client hasn't requested yet. It is **not** a general-purpose server-initiated messaging mechanism (that's WebTransport).

- Handlers attach push hints to Conn via state (e.g., `conn.push_request(Method::Get, "/style.css", Headers::new())`)
- H3 connection handler drains push hints after handler returns, sends PUSH_PROMISE + push streams
- On H1, push hints convert to `Link: </style.css>; rel=preload` headers
- No Handler trait changes needed

## Decision: QUIC traits live in trillium-server-common

QUIC adapter crates (trillium-quinn, etc.) will depend on trillium-server-common. The QUIC traits (`QuicConnection`, `QuicEndpoint`) belong there alongside the existing Server/Runtime abstractions.

## WebTransport (RFC 9220)

Requested in [#687](https://github.com/trillium-rs/trillium/issues/687). WebTransport is to WebSockets what QUIC is to TCP — multiple independent bidirectional streams + unreliable datagrams over a single connection, initiated via HTTP.

### How it works
1. Client sends extended CONNECT with `:protocol: webtransport` over H3
2. Server responds 200 → establishes a WebTransport session
3. Within the session, either side can: open bidirectional streams, open unidirectional streams, send unreliable datagrams
4. Multiple sessions can share one QUIC connection (scoped by session ID)

### What trillium needs
- **Extended CONNECT handling** — recognize `:protocol: webtransport` in H3 requests
- **Session abstraction** — `WebTransportSession` wrapping QUIC stream/datagram APIs, scoped to session
- **Datagram support in QUIC traits** — `QuicConnection` needs `send_datagram()`/`read_datagram()` (not in current sketch)
- **Upgrade-like handler API** — analogous to websocket upgrade:
  ```rust
  let session = trillium_web_transport::upgrade(&conn).await;
  let bi_stream = session.accept_bidi().await;
  session.send_datagram(bytes).await;
  ```
- **Likely a `trillium-web-transport` crate** — parallel to `trillium-websockets`

### Why implement before H3 release
WebTransport is the most likely source of breaking API pressure on the types introduced for H3 (QUIC traits, Conn upgrade mechanism, stream handling). Better to discover those before 0.3 ships. The websocket implementation required upgrade handling APIs that affected other types — this will be similar.

## H3 Client

The H3 client should also be sketched before release. `trillium-client` required significant API changes to accommodate HTTP semantics from the client side, and H3 client will similarly pressure the QUIC trait design and QPACK codec API (client encodes requests, decodes responses — reverse of server). Discovering those tensions early avoids breaking changes post-release.

Key concerns:
- QUIC traits need to work from both connection-accepting (server) and connection-initiating (client) perspectives
- `QuicEndpoint` as currently sketched is server-oriented (`accept`). Client needs a `connect(addr)` method or a separate `QuicConnector` trait.
- Connection pooling — a single QUIC connection can multiplex many requests (no need for connection pools in the TCP sense, but session management is still needed)
- QPACK encoder/decoder roles reverse (client encodes request headers, server decodes them)

## Updated Implementation Sequence

### Phase 1: Shared primitives (in trillium-http, feature-gated)
1. **Huffman codec** — fixed table, encode/decode bytes. ~200-300 lines.
2. **Variable-length integer encoding** — prefix-coded varints. ~50-100 lines.

### Phase 2: QPACK (in trillium-http)
3. **QPACK static table** — 99 entries, hardcoded lookup.
4. **QPACK decoder** — static-table-only, `&mut self` ready for dynamic table.
5. **QPACK encoder** — static-table-only.
6. **Test against RFC 9204 test vectors.**

### Phase 3: H3 framing (in trillium-http)
7. **H3 frame parser/serializer** — varint type + varint length + payload.
8. **H3 SETTINGS handling** — parse/serialize, connection state.

### Phase 4: Conn integration (in trillium-http)
9. **`finalize_headers` version dispatch** — H3 path strips forbidden headers.
10. **`ReceivedBody` DATA frame state** — H3 body alongside Chunked/FixedLength.
11. **Response `Body`** — H3 DATA frames instead of chunked.
12. **Pseudo-header handling** — extract/emit `:method`, `:path`, `:scheme`, `:authority`, `:status`.

### Phase 5: QUIC traits + H3 connection handler (in trillium-server-common)
13. **`QuicConnection` / `QuicEndpoint` traits** — including datagram support for WebTransport.
14. **`QuicConnector` trait** — client-side connection initiation.
15. **H3 connection handler** — accepts streams, runs QPACK + framing, constructs `Conn`, invokes handler.

### Phase 6: quinn adapter (new crate: trillium-quinn)
16. **Implement QUIC traits** for quinn types (both server and client sides).
17. **Runtime shim** — bridge `RuntimeTrait` to quinn's `Runtime`.
18. **UDP socket binding** — Server-level, bridged to quinn's `AsyncUdpSocket`.

### Phase 7: Server wiring
19. **`with_h3()` config** — QUIC provider, UDP listener alongside TCP.
20. **`Alt-Svc` header** — auto-insert on H1 responses.
21. **Graceful shutdown** — Swansong + GOAWAY.

### Phase 8: WebTransport (new crate: trillium-web-transport)
22. **Extended CONNECT handling** — detect and route webtransport upgrades.
23. **Session abstraction** — scoped stream/datagram access.
24. **Upgrade API** — handler-facing interface, parallel to websocket pattern.

### Phase 9: H3 client (in trillium-client)
25. **Client-side QUIC connection management** — using `QuicConnector` trait.
26. **Request encoding / response decoding** — QPACK in client role.
27. **Connection multiplexing** — multiple in-flight requests per QUIC connection.

### Phase 10: Validate + iterate
28. **End-to-end testing** — H3 server + H3 client, WebTransport, interop with browsers/curl.
29. **QPACK dynamic table** — add if compression gains justify complexity.
30. **Server push wiring** — implement Conn-state-based push if there's demand.

## Open Questions

- Exact shape of Server-level UDP/QUIC integration
- How to handle H3 stream interleaving (HEADERS/DATA/trailers) in ReceivedBody
- QPACK dynamic table: when to add (after initial H3 ships? before 1.0?)
- `QuicConnector` trait shape — how does it interact with trillium-client's existing connection management?
- WebTransport session lifecycle — how does graceful shutdown interact with active sessions?
- Does the upgrade mechanism generalize across websockets and WebTransport, or are they separate?
