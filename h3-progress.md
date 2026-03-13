# H3 Implementation Progress

Status as of 2026-03-07.

## What exists

### QPACK (`http/src/headers/qpack/`)

- **`varint.rs`** — HPACK-style prefix-coded varint encode/decode (RFC 7541 §5.1). Used by QPACK field section encoding.
- **`huffman/`** — Huffman encode/decode with compile-time decode tree. Table sourced directly from RFC 7541 appendix.
- **`static_table.rs`** (now `static_table/`) — 99-entry QPACK static table (RFC 9204 appendix A). Forward lookup via `static_entry(index)`, reverse lookup via `static_table_lookup(&HeaderName, &HeaderValue)` (header entries only; pseudo-header lookup inlined in encoder).
- **`static_table/lookup.rs`** — Match-based reverse index for regular headers. Maps `(&HeaderName, &HeaderValue)` → `StaticLookup { FullMatch(u8), NameMatch(u8), NoMatch }`. Uses KnownHeaderName match → index list → value scan against the table.
- **`decoder.rs`** — `decode_field_section()` returns `(PseudoHeaders<'static>, Headers)`. Handles indexed, literal-with-name-ref, and literal-with-literal-name representations. Static table only (dynamic table returns error). `FieldLine` and `PseudoHeader` are internal types used by the three decode helpers. Duplicate pseudo-headers silently ignored (first value wins).
- **`encoder.rs`** — `encode_field_section(&PseudoHeaders, &Headers, buf)` encodes directly from typed pseudo-headers and borrowed headers. Uses Huffman when shorter. Pseudo-header encoding has dedicated functions (`encode_method`, `encode_status`, `encode_pseudo_string`) with inlined static table indices. Regular header encoding uses `static_table_lookup` + `encode_by_lookup`.
- **`PseudoHeaders<'a>`** — Typed struct with the six defined HTTP/3 pseudo-header fields: `method: Option<Method>`, `status: Option<Status>`, `path/scheme/authority/protocol: Option<Cow<'a, str>>`. Implements `Default`. Lifetime parameter allows borrowing from Conn fields during encoding.
- **`tests.rs`** — Round-trip tests using `(PseudoHeaders, &Headers)` interface (all static matches, name-ref with custom value, literal name/value, mixed, all methods, all statuses, non-standard method/status, empty value, long value), RFC 9204 §B.1 decode test, decoder error path tests.

### H3 framing (`http/src/h3/`)

- **`quic_varint.rs`** — QUIC variable-length integer encode/decode (RFC 9000 §16). Different encoding from HPACK varints. `decode` is generic over `T: TryFrom<u64>` so callers get typed results directly (e.g. `decode::<FrameType>(input)`). `encode` to Vec, `encode_to_slice` for borrowed buffers, `encoded_len` for size calculation. `QuicVarIntError` has `UnexpectedEnd` (incomplete) and `UnknownValue { value, rest }` (varint ok, target type mismatch). Core encoding logic shared via `encode_raw`. Tested against RFC 9000 §16 examples.
- **`frame.rs`** — `FrameType` enum (RFC 9114 §7.2), `UniStreamType` enum (RFC 9114 §6.2 + RFC 9204 §4.2). `FrameHeader` (private) handles raw type+length varint parsing. `Frame` enum is the public decode/encode interface — control frames (Settings, Goaway, CancelPush, MaxPushId) are fully parsed/serialized; large-payload frames (Data, Headers, PushPromise, Unknown) only consume/write the frame header, leaving payload for the caller. `FrameDecodeError` unifies incomplete and protocol-error cases. Single-varint frame payloads are validated for exact length. `Frame::encode` writes into `&mut [u8]` with `Option<usize>` return; `encoded_len` precomputes size. Tests in `frame/tests.rs`.
- **`error.rs`** — `H3ErrorCode` enum (RFC 9114 §8.1, all 17 defined codes). `From<u64>` maps all unknown codes to `NoError` per spec. `Into<u64>` emits a random GREASE value (`0x1f * N + 0x21`) for `NoError` using `fastrand`. No GREASE details leak to callers.
- **`settings.rs`** — `H3Settings` struct with `new()` (generates GREASE), builder methods, `decode`/`encode`/`encoded_len`. Decode rejects forbidden H2 setting identifiers (0x00, 0x02–0x05). Unknown identifiers (including GREASE) silently skipped. GREASE id/value stored privately; `PartialEq` ignores them. Now includes `h3_datagram: bool` (RFC 9297 §2.1) and `enable_webtransport: bool` (draft-ietf-webtrans-http3) — only encoded when `true`, decoded as nonzero-is-true.

### Wire format constants (`http/src/headers/qpack.rs`)

Shared constants for QPACK field line type patterns: `INDEXED_FIELD_LINE`, `INDEXED_STATIC_FLAG`, `LITERAL_WITH_NAME_REF`, `NAME_REF_STATIC_FLAG`, `LITERAL_WITH_LITERAL_NAME`. Used by both encoder and decoder.

### Connection management (`http/src/h3/connection.rs`)

- **`H3Connection`** — Per-QUIC-connection state, `Arc`-shared across all stream tasks. Holds `Arc<ServerConfig>`, a connection-scoped `Swansong` (created as a child of `server_config.swansong`), a `OnceLock<H3Settings>` for peer settings, and atomics tracking the max accepted stream ID (for GOAWAY computation). Now public. Public accessors: `swansong()`, `shut_down()` (returns `ShutdownCompletion`), `server_config()`, `peer_settings()`.
- **`run_request`** — Fully implemented. Records the stream ID, takes a swansong guard, allocates a buffer, calls `Conn::new_h3`, runs the handler, calls `send_h3`. Returns `Result<Conn<Transport>, H3Error>` — the caller receives the conn back after the response is sent, enabling upgrade detection.
- **`outbound_control`** — Writes the control stream type varint, sends our SETTINGS frame (derived from `HttpConfig`), then awaits connection shutdown and sends GOAWAY with the correct stream ID.
- **`encoder` / `decoder`** — Write the QPACK encoder/decoder stream type varints and hold streams open until shutdown. Currently idle (static table only).
- **`inbound_uni`** — Reads the stream type varint, dispatches to `inbound_control` for the peer's control stream, holds QPACK streams open, returns an error for unexpected push streams, silently drains unknown stream types per §6.2.
- **`inbound_control`** — Requires first frame to be SETTINGS (stored in `peer_settings`), then loops watching for GOAWAY (triggers `swansong.shut_down()`). Rejects duplicate SETTINGS.
- **Internal `read` / `write` / `drain` helpers** — `read` loops with a grow-on-need buffer, calling a closure that returns `Ok(Some((value, consumed)))` / `Ok(None)` / `Err`. `write` similarly grows the buffer until the closure can encode. Both cap at 10KB. `drain` discards all remaining bytes on a stream.

### Conn H3 integration (`http/src/conn/h3.rs`)

- **`Conn::new_h3`** — Async constructor. Takes `Arc<H3Connection>`. Buffers until a complete HEADERS frame is decoded, QPACK-decodes the field section into `(PseudoHeaders, Headers)`, then calls `build_h3`. Initializes `request_body_state` as `H3Data { remaining_in_frame: 0, frame_type: Start, partial_frame_header: false, total: 0 }`.
- **`Conn::build_h3`** — Fallible constructor. Takes `PseudoHeaders` + `Headers` directly, performs H3 request validation (forbidden connection headers, `Host`/`:authority` mismatch, CONNECT semantics including extended CONNECT with `:protocol`, `TE` check), and builds the Conn. Inserts `Server` header, sets `version: Http3`, `secure: true`.
- **`Conn::send_h3`** — Async response sender. Returns `io::Result<Self>` — the conn is returned after the response is written, enabling upgrade detection. Calls `finalize_headers_h3`, encodes response headers, then copies `H3BodyWrapper` into a `BufWriter`. Skips body for HEAD / 204 / 304 responses.
- **`Conn::encode_headers_h3`** — Constructs `PseudoHeaders { status }`, calls `encode_field_section` with borrowed `&self.response_headers`, validates against peer's `max_field_section_size`, writes the HEADERS frame header + encoded field section into the output buffer.
- **`Conn::finalize_headers_h3`** — Inserts `Date` header (via `try_insert_with`). For non-204/304 responses, inserts `Content-Length` if body length is known. Removes H3-forbidden response headers: `Connection`, `Transfer-Encoding`, `Keep-Alive`, `Proxy-Connection`, `Upgrade`.
- **`Conn::max_peer_field_section_size`** — Helper that reads `peer_settings().max_field_section_size` from the `Arc<H3Connection>`.
- **`Conn::should_upgrade`** — Public method. Returns `true` when `method == CONNECT && status == 200` or `status == SwitchingProtocols`. Used by the server-common layer to detect upgrade requests (WebTransport, etc.).

### Conn struct

Added `authority: Option<Cow<'static, str>>`, `scheme: Option<Cow<'static, str>>`, and `protocol: Option<Cow<'static, str>>` fields to hold H3 pseudo-header values. Added `h3_connection: Option<Arc<H3Connection>>` field; set to `Some` for H3 conns, `None` for H1.

### Upgrade struct

Added `authority`, `scheme`, `h3_connection`, and `protocol` fields. `From<Conn<T>> for Upgrade<T>` carries all H3 fields through, making Upgrade suitable for both H1 WebSocket upgrades and H3 WebTransport upgrades.

### `H3Error` (`http/src/h3.rs`)

Public error type `H3Error`. `Protocol(H3ErrorCode)` for H3 protocol errors. `Io(io::Error)` for unrecoverable network errors.

### H3 outbound body framing (`http/src/h3/body_wrapper.rs`)

`H3BodyWrapper` wraps a `BodyType` and implements `AsyncRead`, inserting DATA frame headers inline. Three cases:

- **`BodyType::Empty`** — returns 0 immediately.
- **`BodyType::Static` (known length)** — on the first poll, writes a single `DATA(len)` frame header into the front of `buf`, then copies as many body bytes as fit. Sets `header_written = true` so subsequent polls are pure passthrough.
- **`BodyType::Streaming { len: Some(_) }` (known length)** — same single-header strategy but for streaming content. On the first poll, reserves `frame.encoded_len()` bytes at the front, reads body bytes into the remainder, then writes the frame header in-place before returning. Subsequent polls pass through body bytes directly.
- **`BodyType::Streaming { len: None }` (unknown length)** — per-poll framing. Reserves `Frame::Data(buf.len()).encoded_len()` bytes (worst-case header size) at the front of `buf`, reads body bytes into the remainder, then encodes the actual `DATA(bytes)` header and `copy_within`s the payload down if the real header was shorter than the reservation.

### Inbound H3 body decoding (`ReceivedBodyState::H3Data`, `http/src/received_body/h3_data.rs`)

`ReceivedBodyState::H3Data` tracks: `remaining_in_frame: u64`, `total: u64` (body bytes across all DATA frames), `frame_type: H3BodyFrameType`, `partial_frame_header: bool`.

`H3BodyFrameType` enum: `Start` (no frame yet), `Data` (keep bytes), `Unknown` (discard bytes), `Trailers` (accumulate into `self.buffer` for future QPACK decode).

`handle_h3_data` dispatches on `partial_frame_header`:

- **Normal path** — calls `read_raw` (buffer-first transport read), then delegates to `h3_frame_decode`. Stream FIN (`bytes == 0`) transitions to `End`, validating content-length if present.
- **Partial header path** — the buffer holds an incomplete frame header from the previous read. Reads more bytes from transport into buf, appends to `self.buffer`, retries `Frame::decode`. On still-incomplete: stays in partial state. On success: advances `self.buffer` past the consumed bytes, copies any payload already in the buffer into `buf`, then falls through to `h3_frame_decode`.

`h3_frame_decode` (free function) processes a filled `buf` in a loop:

1. Consumes `remaining_in_frame` bytes according to `frame_type`: DATA ranges are collected into `ranges_to_keep`; Trailers bytes are appended to `self_buffer`; Unknown bytes are discarded. Validates total ≤ max_len and total ≤ content-length mid-stream. When a Trailers frame completes, validates final content-length and breaks to `End`.
2. Decodes the next frame header from `buf[pos..]`. On `Incomplete`, saves the partial header bytes into `self_buffer` and sets `partial_frame_header: true`. On frame error or unexpected frame type, returns `Err`.
3. After the loop, compacts `buf` in-place using `copy_within` over `ranges_to_keep`, returning the total body byte count.

23 tests in `h3_data/tests.rs` covering: sync decode with single/multiple DATA frames, frame boundaries split across reads, partial frame headers, unknown frame skip, trailers termination, content-length validation (mid-stream excess, final mismatch), max-len enforcement, async reads with various buffer and frame sizes, and stream FIN handling.

### Round-trip tests (`http/src/h3/tests.rs`)

7 tests using `TestTransport` pairs to drive `H3BodyWrapper` → wire → `ReceivedBody<H3Data>` end-to-end:

- **`empty_body`** — empty body round-trips to empty string.
- **`static_body`** — static body with known content-length.
- **`streaming_known_length`** — streaming body with `len: Some(_)`.
- **`streaming_unknown_length`** — streaming body with `len: None` (per-poll framing).
- **`static_body_various_buf_sizes`** — drives `H3BodyWrapper` with buf sizes from 3 through body_len+4, asserting correct decode at each size.
- **`streaming_known_length_various_buf_sizes`** — same sweep for streaming known-length.
- **`streaming_unknown_length_various_buf_sizes`** — same sweep for streaming unknown-length (exercises multi-frame paths when buf is smaller than the body).

### `Version::Http3`

Added to the `Version` enum with full `Display`, `FromStr` (`"HTTP/3"`, `"http/3"`, `"3"`), `as_str`, `Ord`, and serialization support.

## Wire protocol status

The wire protocol layer is complete for static-table-only QPACK and all H3 frame types. Tests cover encode/decode roundtrips, error cases, incomplete input handling, GREASE, and spec compliance.

## Server integration (`server-common/`)

### QUIC trait hierarchy (`server-common/src/quic.rs`)

Three traits form the configuration → binding → connection chain:

- **`QuicConfig<S: Server>`** — User-provided QUIC configuration. Generic over `Server` so `bind` receives the runtime. Has `bind(SocketAddr, S::Runtime) -> Option<io::Result<Self::Binding>>`. The `()` impl returns `None` (HTTP/3 disabled).
- **`QuicBinding`** — A bound QUIC endpoint (e.g. wraps `quinn::Endpoint`). Has `accept() -> Option<Self::Connection>`. The `()` impl returns `None` immediately.
- **`QuicConnection`** — A single QUIC connection (e.g. wraps `quinn::Connection`). Three associated stream types (`BidiStream: Transport`, `RecvStream: AsyncRead`, `SendStream: AsyncWrite`). Ten methods: `accept_bidi`, `accept_uni`, `open_uni`, `remote_address`, `close`, `stop_stream`, `send_datagram`, `recv_datagram`, `max_datagram_size`.

**`NoQuic`** — Uninhabited enum implementing `QuicConnection`, `AsyncRead`, `AsyncWrite`, and `Transport` via `match *self {}`. Used as the connection type for the `()` `QuicBinding`.

### UDP transport (`server-common/src/udp_transport.rs`)

**`UdpTransport`** trait — async UDP socket abstraction for QUIC. Closure-based API designed so implementations manage readiness state internally:

- `poll_recv_io(&self, cx, recv: impl FnMut(&Self) -> io::Result<R>) -> Poll<io::Result<R>>` — polls for readability, then calls `recv` with `&self`. On `WouldBlock`, clears readiness and re-polls. The closure receives `&Self` so it can access platform-specific traits (e.g. `AsFd`) without those traits appearing in this trait's definition.
- `poll_writable(&self, cx) -> Poll<io::Result<()>>` — polls for write readiness without performing I/O. Separated from send because QUIC libraries may have multiple concurrent senders per socket.
- `try_send_io(&self, send: impl FnOnce(&Self) -> io::Result<R>) -> io::Result<R>` — attempts a send, managing readiness clearing if needed.
- `from_std(UdpSocket)`, `local_addr()`, plus optional `max_transmit_segments`, `max_receive_segments`, `may_fragment` for GSO/GRO.

The `()` impl returns errors from all methods. `Server` trait has `type UdpTransport: UdpTransport`.

**Runtime implementations:**
- **`TokioUdpSocket`** (`tokio/src/udp.rs`) — wraps `tokio::net::UdpSocket`. Uses `poll_recv_ready` + `try_io` for readiness management. Implements `AsFd` (unix) and `AsSocket` (windows).
- **`SmolUdpSocket`** (`smol/src/udp.rs`) — wraps `async_io::Async<UdpSocket>`. Edge-triggered readiness via `poll_readable`/`poll_writable`. Implements `AsFd` (unix) and `AsSocket` (windows).
- **`AsyncStdUdpSocket`** (`async-std/src/udp.rs`) — same as smol (both use `async-io`). Implements `AsFd` (unix) and `AsSocket` (windows).

### Config integration (`server-common/src/config.rs`)

`Config` gained a third generic parameter: `Config<ServerType, AcceptorType, QuicType: QuicConfig = ()>`. New method `with_quic(q)` swaps the `QuicType` generic, mirroring `with_acceptor`. In `run_async`:

1. QUIC binding happens after listener init (socket address available), before `ServerConfig` is constructed
2. If no `SocketAddr` in info (UDS case), QUIC binding is skipped
3. If binding is configured, panics on bind failure (fail-fast during setup)
4. Handler is wrapped in `ArcHandler` and shared between H1 and H3 tasks
5. H3 task is spawned before the H1 accept loop starts

### H3 connection handler (`server-common/src/h3.rs`)

**`run_h3`** — Top-level accept loop. Accepts `QuicConnection`s from the `QuicBinding`, creates an `H3Connection` per peer, spawns `run_h3_connection` per connection. Uses `swansong.interrupt()` to stop accepting on shutdown.

**`run_h3_connection`** — Per-connection handler, generic over `QC: QuicConnection`. Spawns 4 sub-tasks (outbound control, QPACK encoder, QPACK decoder, inbound uni accept loop), then runs the bidirectional request accept loop inline. Each spawned task calls `handle_h3_error` on failure, which logs, closes the QUIC connection with the error code (for protocol errors), and shuts down the connection's swansong.

The bidi request loop calls `H3Connection::run_request` with a handler closure that sets peer IP, marks the conn as secure, inserts the `QuicConnection` as state (accessible to handlers), and runs the handler's `run` + `before_send`. After `run_request` returns the conn, the caller checks `should_upgrade()` — if true, converts `Conn` → `Upgrade` (boxing the transport), and calls `handler.has_upgrade()` / `handler.upgrade()`, mirroring the H1 upgrade path. This enables WebTransport and other H3 upgrade protocols.

### HttpConfig

Added `webtransport_enabled: bool` and `h3_datagrams_enabled: bool` fields, wired through `From<&HttpConfig> for H3Settings`. These are set by handlers during `init` via `Info::http_config_mut()`.

## trillium-quinn (`quinn/`)

Runtime-agnostic quinn adapter crate. All H3 orchestration logic lives in server-common; trillium-quinn is a thin bridge between quinn's types and trillium's QUIC traits.

### Runtime shim (`quinn/src/runtime.rs`)

**`TrilliumRuntime<R, U>`** — implements `quinn::Runtime` by bridging to trillium's `RuntimeTrait` + `UdpTransport`:
- `spawn` → drops the `DroppableFuture` (detach-on-drop semantics match quinn's fire-and-forget spawn)
- `new_timer` → `Timer<R>`, a resettable timer that replaces its inner boxed delay future on each `reset`
- `wrap_udp_socket` → constructs `UdpSocket<U>` wrapping a `UdpTransport` + quinn-udp's `UdpSocketState`

**`SocketTransport`** — platform-conditional trait alias (`UdpTransport + AsFd` on unix, `UdpTransport + AsSocket` on windows). quinn-udp's `UdpSocketState` needs raw fd/socket access for platform-optimized syscalls. This bound stays inside trillium-quinn; the public `UdpTransport` trait in server-common is platform-agnostic.

**`UdpSocket<U>`** — implements `quinn::AsyncUdpSocket`:
- `poll_recv` → `transport.poll_recv_io(cx, |t| inner.recv(UdpSockRef::from(t), bufs, meta))`
- `try_send` → `transport.try_send_io(|t| inner.send(UdpSockRef::from(t), transmit))`
- `create_io_poller` → `UdpPoller` wrapping `transport.poll_writable`
- GSO/GRO/fragmentation delegated to `UdpSocketState`

### Connection types (`quinn/src/connection.rs`)

- **`QuinnTransport`** — combines quinn's `SendStream` + `RecvStream` into a single `Transport` via derive macros. Streams wrapped in `async_compat::Compat` to bridge tokio ↔ futures-lite I/O traits.
- **`QuinnConnection`** — wraps `quinn::Connection`, implements `QuicConnection`. Includes datagram support: `send_datagram` (sync, via `Bytes::copy_from_slice`), `recv_datagram` (async, appends to `impl Extend<u8>`), `max_datagram_size`.

### Config + binding (`quinn/src/config.rs`)

- **`QuicConfig`** — user-facing struct. `from_single_cert(cert_pem, key_pem)` builds a `quinn::ServerConfig` with `h3` ALPN. Also accepts pre-built configs via `from_quinn_server_config`.
- `impl<S: Server> QuicConfigTrait<S> for QuicConfig` where `S::Runtime: Unpin, S::UdpTransport: SocketTransport` — `bind` constructs `TrilliumRuntime`, binds a UDP socket, creates `quinn::Endpoint::new`. Now unconditional (not `#[cfg(unix)]`).
- **`QuinnBinding`** — wraps `quinn::Endpoint`, implements `QuicBinding`. Accept loop retries on individual connection handshake failures.

### User-facing API

```rust
trillium_tokio::config()
    .with_acceptor(RustlsAcceptor::from_single_cert(&cert_pem, &key_pem))
    .with_quic(trillium_quinn::QuicConfig::from_single_cert(&cert_pem, &key_pem))
    .run(handler);
```

The `Server` type parameter is inferred — users never name it. Works with any trillium runtime whose `UdpTransport` implements `AsFd` (unix) or `AsSocket` (windows).

### Design direction

- **trillium-http** owns H3 protocol logic (framing, QPACK, stream types) but is runtime-agnostic. No task spawning.
- **trillium-server-common** owns the generic H3 connection handler and QUIC trait definitions. Spawns tasks via the type-erased `Runtime`.
- **QUIC library adapters** (trillium-quinn, etc.) implement the three QUIC traits. Thin shims — all H3 orchestration logic is shared.
- **Runtime adapters** implement `UdpTransport`. Can opt out with `type UdpTransport = ()`.

### Connection architecture

```
ServerConfig (per server, Arc-shared across all connections)
  └── H3Connection (per QUIC connection, Arc-shared across streams)
       └── request stream task (per request, holds Arc<H3Connection>)
```

- **`run_h3` task**: accepts QUIC connections from the binding, spawns per-connection handlers.
- **`run_h3_connection` task**: opens control/encoder/decoder uni streams, accepts inbound uni streams, accepts bidi request streams. All sub-tasks share the `H3Connection` via `Arc`.
- **Request stream tasks**: hold one bidirectional QUIC stream + `Arc<H3Connection>`. Read HEADERS frame, QPACK-decode, build Conn, run handler, write response HEADERS + DATA frames. After response, check `should_upgrade()` for WebTransport/etc.
- **QPACK state**: currently stateless (static table only). When dynamic table is added, shared state will be `Arc<RwLock<_>>` — request streams await the insert count they need, then read-lock the table for header lookups.

## Design decisions made during wire protocol implementation

- **`FrameHeader` is private** — `Frame` enum is the only public framing interface. Each layer fully hides its internals.
- **`Frame::encode` writes into `&mut [u8]`** — avoids allocation; caller provides buffer from I/O layer. Returns `None` if buffer too small.
- **`FrameDecodeError`** — unified error type with `Incomplete` and `Error(H3ErrorCode)` variants, replacing `Option<Result>`. `From<H3ErrorCode>` enables `?` propagation.
- **GREASE stored in `H3Settings`** — random values chosen at construction (`new()`), ensuring `encoded_len()` and `encode()` agree. Decoded settings have grease=0.
- **`H3ErrorCode` uses `From<u64>`/`Into<u64>`** — all unknown error codes map to `NoError` per spec; `NoError` encodes as a random GREASE value. No GREASE logic leaks to callers.
- **Hardcoded limits for control frames, configurable `max_field_section_size` for HEADERS** — adversarial payload sizes handled one layer up from the parsers.
- **H3 body = sequence of DATA frames** — no chunked encoding equivalent; QUIC FIN signals end of body. Sender emits DATA frames of convenient size.
- **H3 upgrade reuses H1 upgrade mechanism** — `send_h3` returns the conn, `should_upgrade()` checks `CONNECT+200` or `SwitchingProtocols`, server-common converts to `Upgrade` and calls `handler.has_upgrade()`/`handler.upgrade()`. Same flow for both protocols.
- **WebTransport/datagram settings as `bool`** — `h3_datagram` and `enable_webtransport` are plain `bool` (not `Option<bool>`) since the default is `false` and there's no distinction between absent and explicitly-false. Only encoded when `true`.

## Completed cleanup

- **`PseudoHeaders<'a>` struct** — Replaced `FieldLine` enum + `PseudoHeader` enum + `PseudoHeaderName` enum as the public QPACK interface. Typed struct with the six defined pseudo-header fields (`:method`, `:status`, `:path`, `:scheme`, `:authority`, `:protocol`). `FieldLine` and `PseudoHeader` are now purely internal to the decoder's parse helpers.
- **Decoder returns `(PseudoHeaders<'static>, Headers)`** — instead of `Vec<FieldLine>`. Pseudo-headers are decoded directly into the struct; regular headers go into `Headers`. No intermediate allocation.
- **Encoder takes `(&PseudoHeaders<'_>, &Headers)`** — instead of `&[FieldLine]`. Borrows response headers in place, no `std::mem::take` or cloning. Pseudo-header static table lookup inlined in the encoder.
- **`ConnParts` eliminated** — `build_h3` takes `PseudoHeaders` + `Headers` directly and is fallible, combining validation and construction.
- **Static table `Option` removal** — `Option<&'static str>` → `&'static str` (name-only entries use `""`). `FieldLine::Header` changed from `Option<HeaderValue>` to `HeaderValue`.
- **Encoder allocation removal** — `field_line_value_bytes` and `field_line_name_value_bytes` return `&[u8]` instead of `Vec<u8>`, using `Status::code()` for status encoding.

## What's next

Priority order reflects API-stability risk — items that might force changes to existing types come first.

### TLS credential sharing (high priority — API design risk)

Currently the user passes cert/key PEM separately to `RustlsAcceptor::from_single_cert` and `trillium_quinn::QuicConfig::from_single_cert`. This is redundant and becomes untenable with ACME (trillium-acme), where certificates are provisioned dynamically. Analysis suggests this is purely additive to the current public API — `QuicConfig::from_rustls_config` for static certs, config stream for ACME. See h3-planning.md for full design.

### WebTransport (trillium-web-transport crate)

Foundation work done: upgrade mechanism wired for H3 (CONNECT+200 detected, `Upgrade` carries H3 fields), datagram support on `QuicConnection`, SETTINGS for `H3_DATAGRAM` and `ENABLE_WEBTRANSPORT`. Remaining:
- Extended CONNECT handling (`:protocol: webtransport` recognition in request routing)
- Session abstraction scoping streams/datagrams to a session ID (CONNECT stream's quarter-stream-ID)
- `open_bi` on `QuicConnection` (needed for handler-initiated streams within a session)
- Handler API (start simple: closure receiving a session object)

### H3 client (trillium-client)

Client-side QUIC connection management. QPACK encoder/decoder roles reverse. `QuicConnector` trait (client-side counterpart to `QuicConfig`/`QuicBinding`). Connection multiplexing. See h3-planning.md for design notes.

### Remaining server integration work

- **Graceful shutdown** — verify GOAWAY + drain works correctly with the swansong hierarchy (server → connection → request)
- **Error handling audit** — ensure all quinn error types are mapped appropriately (connection errors, stream errors, TLS errors)
- **Connection limits** — `max_connections` currently only applies to TCP; should it also govern QUIC connections?
- **Logging** — consistent log output for QUIC connection lifecycle events

### Lower priority

- **QPACK dynamic table** — add when compression gains justify complexity
- **Server push** — implement Conn-state-based push if there's demand
