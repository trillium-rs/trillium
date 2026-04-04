# WebTransport Implementation Design

## Wire Protocol

WebTransport over HTTP/3 is thin — it's mostly raw QUIC streams/datagrams with
a session ID prefix.

### Session establishment

Normal H3 extended CONNECT: `:method: CONNECT`, `:protocol: webtransport`,
`:authority` + `:path` identify the endpoint. Server responds 200. The CONNECT
stream's QUIC stream ID becomes the session ID.

### Bidi streams

Wire format: `varint(0x41) + varint(session_id) + payload`

0x41 is the WT_STREAM signal value, registered as an H3 frame type but NOT a
proper H3 frame (no length field). Since 0x41 = 65 > 63, the varint encoding
is 2 bytes: `[0x40, 0x41]`. The existing `FrameHeader::decode` naturally parses
this as `frame_type=0x41, payload_length=session_id` — we added
`FrameType::WebTransport = 0x41` and `Frame::WebTransport(session_id)` to
handle it.

### Uni streams

Wire format: `varint(0x54) + varint(session_id) + payload`

0x54 is the WT uni stream type. Fits into the existing `UniStreamType` dispatch
in `H3Connection::inbound_uni`. Added `UniStreamType::WebTransport = 0x54`.

### Datagrams

Quarter-stream-ID varint prefix (session_id / 4), then payload. Uses the QUIC
DATAGRAM frame, gated by `H3_DATAGRAM` setting (already wired).

## Architecture

### Design principles

**trillium-http parses; server-common routes; trillium-webtransport handles.**
Each layer returns data to its caller rather than taking callbacks or closures.
H3Connection has no WebTransport-specific state.

**Return enums, not callbacks.** The H3 layer returns `H3StreamResult` and
`UniStreamResult` — the caller decides what to do with WebTransport streams.
This removed all `AsyncFn` callback parameters from the H3 interface and keeps
the stop/reject decision with the code that has the concrete QUIC types.

### Stream routing (bidi)

The first varint on a client-initiated bidi stream discriminates:
`0x01` (HEADERS) = H3 request, `0x41` (WT_STREAM) = WebTransport.

`Conn::new_h3` reads this and returns `H3StreamResult<Transport>`:
```rust
pub enum H3StreamResult<Transport> {
    Request(Conn<Transport>),
    WebTransport { session_id: u64, transport: Transport, buffer: Buffer },
}
```

`H3Connection::run_request` propagates this — it only invokes the handler
closure for the `Request` variant. Generic over `Transport` throughout
trillium-http; type erasure happens at the server-common boundary.

### Stream routing (uni)

`H3Connection::inbound_uni` reads the stream type varint and returns
`UniStreamResult<T>`:
```rust
pub enum UniStreamResult<T> {
    Handled,  // control, QPACK — managed internally
    WebTransport { session_id: u64, stream: T, buffer: Buffer },
    Unknown { stream_type: u64, stream: T },
}
```

For the `WebTransport` variant, the session ID varint is also consumed before
returning. The stream and buffer contain only application payload bytes.

### WebTransportDispatcher (server-common)

Per-QUIC-connection dispatcher, created when `config().webtransport_enabled()`.
Lives in `server-common/src/h3/web_transport.rs`.

```rust
pub struct WebTransportDispatcher(Arc<RwLock<WebTransportDispatch>>);

enum WebTransportDispatch {
    Buffering(Vec<WebTransportStream>),
    Active(Box<dyn Fn(WebTransportStream) + Send + Sync>),
}
```

- Created in `run_h3_connection` alongside `H3Connection`.
- `dispatch()` uses a read lock fast path when Active (common case), falls
  through to write lock when still Buffering (rare early-arrival case).
- `set_handler()` atomically transitions Buffering → Active and drains buffered
  streams through the new handler.
- Inserted into each Conn's state so the WebTransport handler can retrieve it.

`WebTransportStream` uses type-erased types (`Box<dyn Transport>`, `Box<dyn AsyncRead>`,
`Vec<u8>` for buffer) since it crosses the crate boundary.

### Early stream buffering

The RFC says clients MAY optimistically open streams before receiving the 200
response (client MAY, not MUST). Endpoints SHOULD buffer, MUST limit, and can
reject with `WT_BUFFERED_STREAM_REJECTED`.

The dispatcher starts in `Buffering` state. Streams that arrive before the
handler registers are held in a small Vec. This is cheap — QUIC streams are
virtual state machines, not file descriptors. When the handler registers via
`set_handler`, buffered streams are drained through it.

If WebTransport is not enabled at the config level, no dispatcher is created
and WT streams are stopped immediately with `StreamCreationError`.

### Datagrams

Datagrams bypass H3 framing entirely — raw QUIC datagrams with a
quarter-stream-ID varint prefix. The WebTransport handler owns datagram
routing: it loops on `QuicConnection::recv_datagram()`, parses the
quarter-stream-ID, and routes to the appropriate session. This keeps datagram
handling entirely in trillium-webtransport with no changes to trillium-http.

Per-session datagram delivery uses a bounded channel with drop-on-overflow
semantics (datagrams are unreliable, recency > completeness).

### Handler lifecycle

1. CONNECT request arrives → normal H3 request → Conn constructed
2. Conn state contains `WebTransportDispatcher` and `QuicConnection`
3. WebTransport handler sees CONNECT + `:protocol: webtransport` → responds 200
4. `should_upgrade()` → true → Upgrade created
5. In `upgrade()`, handler retrieves `WebTransportDispatcher` from state, calls
   `set_handler(move |stream| sender.try_send(stream))` with a channel sender
6. Handler creates a WebTransportConnection for this session, holding the
   receiver end and demuxing by session_id
7. Subsequent WT streams: dispatcher calls the handler fn, which sends through
   the channel. WebTransport handler demuxes by session_id.

### Ownership and cleanup

- Dispatcher is Arc-shared across all stream tasks for one QUIC connection
- When the QUIC connection drops, the dispatcher drops, sender drops, receiver
  sees channel closed
- No global connection table needed

### Where code lives

- **trillium-http**: Frame types (0x41, 0x54), `H3StreamResult`,
  `UniStreamResult`, `H3ErrorCode` variants for WT errors. Session ID varint
  parsing for both bidi and uni streams. No WT-specific state on H3Connection.
- **trillium-server-common**: `WebTransportDispatcher`, `WebTransportStream`
  enum (with boxed types), stream routing in `run_h3_connection`, dispatcher
  creation and insertion into Conn state.
- **trillium-webtransport**: Channel management, session demuxing, handler API,
  datagram routing, Stream abstractions for user code.

## Trait changes (cumulative)

### QuicConnectionTrait (renamed from QuicConnection)
- `accept_bi` → `accept_bidi`
- Added `open_bidi`
- All accept/open methods return `(u64, stream_type)` for stream ID access
- `recv_datagram` returns `Vec<u8>` instead of taking `&mut impl Extend`
- `stop_stream` renamed to `stop_uni`
- Added `stop_bidi` (sends STOP_SENDING + RESET_STREAM)
- `stop_uni`/`stop_bidi` omitted from type-erased `QuicConnection`

### QuicConnection (type-erased)
- `Arc<dyn ObjectSafeQuicConnection>`, cheaply cloneable
- Constructed via `QuicConnection::from(impl QuicConnectionTrait)`
- Inserted into conn state by the H3 handler in server-common

### H3 error codes
Added WebTransport error codes to `H3ErrorCode`:
- `WebTransportBufferedStreamRejected` (0x3994bd84)
- `WebTransportSessionGone` (0x170d7b68)
- `WebTransportFlowControlError` (0x045d4487)
- `WebTransportAlpnError` (0x0817b3dd)
- `WebTransportRequirementsNotMet` (0x212c0d48)

## Next steps

1. Build the trillium-webtransport handler: channel pair, session demux,
   upgrade flow using `WebTransportDispatcher::set_handler`
2. Build `WebTransportConnection` with Stream-based inbound API for bidi/uni
3. Datagram routing loop in the WT handler
4. Echo server example + JS client to validate
5. Consider buffer cap enforcement in the dispatcher

## Open questions

- Multiple sessions per connection: the channel carries session_id, handler
  demuxes. What about per-session stream limits (WT_MAX_STREAMS capsules)?
- Session cleanup when a session ends but the connection stays alive
- Does the upgrade mechanism generalize across websockets and WebTransport,
  or are they necessarily separate?
- BoxedQuicBidi / BoxedQuicUni types for stop support from the WT handler
  (currently stop is only available with concrete types)
