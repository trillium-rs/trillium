use fieldwork::Fieldwork;

/// # Performance and security parameters for trillium-http.
///
/// Trillium's http implementation is built with sensible defaults, but applications differ in usage
/// and this escape hatch allows an application to be tuned. It is best to tune these parameters in
/// context of realistic benchmarks for your application.
#[derive(Clone, Copy, Debug, Fieldwork)]
#[fieldwork(get, get_mut, set, with, without)]
// `HttpConfig` is a user-facing tuning struct with documented per-field setters; the natural
// shape is one field per knob. Bundling bools into an enum or bitflags would make the getter/
// setter surface worse for callers.
#[allow(clippy::struct_excessive_bools)]
pub struct HttpConfig {
    /// The maximum length allowed before the http body begins for a given request.
    ///
    /// **Default**: `8kb` in bytes
    ///
    /// **Unit**: Byte count
    pub(crate) head_max_len: usize,

    /// The maximum length of a received body
    ///
    /// This limit applies regardless of whether the body is read all at once or streamed
    /// incrementally, and regardless of transfer encoding (chunked or fixed-length). The correct
    /// value will be application dependent.
    ///
    /// **Default**: `10mb` in bytes
    ///
    /// **Unit**: Byte count
    pub(crate) received_body_max_len: u64,

    /// The initial capacity of the buffer that serializes the response head and batches body
    /// writes.
    ///
    /// The response head and as much of the body as fits are coalesced into a single transport
    /// write, so a response up to a few KiB is sent in one write regardless of this value. It
    /// matters only for large response bodies, where a larger buffer batches more bytes per write
    /// and so reduces the number of write syscalls on a bulk transfer — at the cost of that much
    /// initial memory per connection. On the common path the buffer flushes when full rather than
    /// growing; `response_buffer_max_len` bounds only the separate backpressure-absorption path.
    ///
    /// **Default**: `512`
    ///
    /// **Unit**: byte count
    pub(crate) response_buffer_len: usize,

    /// Maximum size the response buffer may grow to absorb backpressure.
    ///
    /// When the transport cannot accept data as fast as the response body is produced, the buffer
    /// absorbs the remainder up to this limit. Once the limit is reached, writes apply
    /// backpressure to the body source. This prevents a slow client from causing unbounded memory
    /// growth.
    ///
    /// **Default**: `2mb` in bytes
    ///
    /// **Unit**: byte count
    pub(crate) response_buffer_max_len: usize,

    /// The initial buffer allocated for the request headers.
    ///
    /// Ideally this is the length of the request headers. It will grow nonlinearly until
    /// `head_max_len` or the end of the headers are reached, whichever happens first.
    ///
    /// **Default**: `1024`
    ///
    /// **Unit**: byte count
    pub(crate) request_buffer_initial_len: usize,

    /// The expected number of response headers, used to size the response header map on conn
    /// creation.
    ///
    /// The map grows on insertion beyond this. The value is split evenly across two internal
    /// stores, so prefer to overestimate — an undersized map reallocates as it fills.
    ///
    /// **Default**: `32`
    ///
    /// **Unit**: Header count
    pub(crate) response_header_initial_capacity: usize,

    /// The expected number of request headers, used to size the request header map while parsing.
    ///
    /// The map grows on insertion beyond this. The value is split evenly across two internal
    /// stores, so prefer to overestimate — an undersized map reallocates as it fills, which is the
    /// dominant cost of building the header map for header-heavy requests.
    ///
    /// **Default**: `32`
    ///
    /// **Unit**: Header count
    pub(crate) request_header_initial_capacity: usize,

    /// Cooperative task-yielding knob.
    ///
    /// Decreasing this number will improve tail latencies at a slight cost to total throughput for
    /// fast clients. This will have more of an impact on servers that spend a lot of time in IO
    /// compared to app handlers.
    ///
    /// **Default**: `16`
    ///
    /// **Unit**: the number of consecutive `Poll::Ready` async writes to perform before yielding
    /// the task back to the runtime.
    pub(crate) copy_loops_per_yield: usize,

    /// The initial buffer capacity allocated when reading a chunked http body to bytes or string.
    ///
    /// Ideally this would be the size of the http body, which is highly application dependent. As
    /// with other initial buffer lengths, further allocation will be performed until the necessary
    /// length is achieved. A smaller number will result in more vec resizing, and a larger number
    /// will result in unnecessary allocation.
    ///
    /// **Default**: `128`
    ///
    /// **Unit**: byte count
    pub(crate) received_body_initial_len: usize,

    /// Maximum size to pre-allocate based on content-length for buffering a complete request body
    ///
    /// When we receive a fixed-length (not chunked-encoding) body that is smaller than this size,
    /// we can allocate a buffer with exactly the right size before we receive the body.  However,
    /// if this is unbounded, malicious clients can issue headers with large content-length and
    /// then keep the connection open without sending any bytes, allowing them to allocate
    /// memory faster than their bandwidth usage. This does not limit the ability to receive
    /// fixed-length bodies larger than this, but the memory allocation will grow as with
    /// chunked bodies. Note that this has no impact on chunked bodies. If this is set higher
    /// than the `received_body_max_len`, this parameter has no effect. This parameter only
    /// impacts [`ReceivedBody::read_string`](crate::ReceivedBody::read_string) and
    /// [`ReceivedBody::read_bytes`](crate::ReceivedBody::read_bytes).
    ///
    /// **Default**: `1mb` in bytes
    ///
    /// **Unit**: Byte count
    pub(crate) received_body_max_preallocate: usize,

    /// The maximum cumulative size of a header block the peer may send.
    ///
    /// Advertised in SETTINGS as `SETTINGS_MAX_HEADER_LIST_SIZE` on HTTP/2 (RFC 9113) and
    /// `SETTINGS_MAX_FIELD_SECTION_SIZE` on HTTP/3 (RFC 9114). Guards against pathological
    /// header lists inflating memory per stream during HPACK/QPACK decode.
    ///
    /// On HTTP/2 this also bounds the cumulative compressed bytes of a header block
    /// accumulated across HEADERS + CONTINUATION frames: a block exceeding this limit closes
    /// the connection with `ENHANCE_YOUR_CALM`, mitigating the CONTINUATION-flood `DoS`
    /// (CVE-2024-27316 class). Otherwise the peer is expected to self-police.
    ///
    /// **Default**: `32 KiB` in bytes
    ///
    /// **Unit**: byte count
    pub(crate) max_header_list_size: u64,

    /// Maximum capacity of the dynamic header-compression table.
    ///
    /// Advertised to peers as `SETTINGS_HEADER_TABLE_SIZE` (HPACK / RFC 7541) and
    /// `SETTINGS_QPACK_MAX_TABLE_CAPACITY` (QPACK / RFC 9204). Bounds both the decoder's
    /// inbound table and our encoder's outbound table; set to `0` to disable dynamic-table
    /// compression entirely (encoder reduces to static-or-literal).
    ///
    /// **Default**: `4 KiB` in bytes
    ///
    /// **Unit**: Byte count
    pub(crate) dynamic_table_capacity: usize,

    /// Maximum number of HTTP/3 request streams that may be blocked waiting for dynamic table
    /// updates.
    ///
    /// Advertised to peers as `SETTINGS_QPACK_BLOCKED_STREAMS`. A value of `0` prevents peers
    /// from sending header blocks that reference table entries not yet seen by this decoder.
    ///
    /// **Default**: 100
    ///
    /// **Unit**: Stream count
    pub(crate) h3_blocked_streams: usize,

    /// Per-connection ring size for the header encoder's recently-seen-pair predictor.
    ///
    /// Applies to both HPACK (HTTP/2) and QPACK (HTTP/3). The predictor lets the encoder
    /// defer dynamic-table inserts until a `(name, value)` pair has been seen at least
    /// once on the connection — first sighting emits a literal, subsequent sightings
    /// within the ring's retention window invest in an insert so future sections can
    /// index it. A larger ring catches repetitions across more intervening header lines
    /// (good for header-heavy reverse proxies); a smaller ring forgets faster (fine for
    /// tiny APIs). A cross-connection observer short-circuits this for already-known-hot
    /// pairs.
    ///
    /// The predictor is consulted once per emitted header line via a u32 hash compare;
    /// cost grows linearly with `size` but is dominated by the per-line hash, so
    /// oversizing here is cheap.
    ///
    /// **Default**: 64
    ///
    /// **Unit**: Pair count
    pub(crate) recent_pairs_size: usize,

    /// Initial HTTP/2 stream flow-control window advertised to peers as
    /// `SETTINGS_INITIAL_WINDOW_SIZE` — the lower tier of the two-tier per-stream window.
    ///
    /// Controls how many request-body bytes the peer may send on a newly-opened stream before the
    /// handler starts reading. Once the handler signals intent to read (first `poll_read` on the
    /// request body), the window is promoted to `h2_max_stream_recv_window_size`; a stream whose
    /// handler never reads the body stays at this initial.
    ///
    /// Must not exceed `2^31 - 1`.
    ///
    /// **Default**: `256 KiB`
    ///
    /// **Unit**: byte count
    pub(crate) h2_initial_stream_window_size: u32,

    /// Per-stream recv window target — the upper tier of the two-tier window. A stream opens at
    /// `h2_initial_stream_window_size` and is promoted to this value once the handler signals
    /// intent to read the request body (first `poll_read`); the driver then tops the peer's window
    /// back up to it via `WINDOW_UPDATE` as the handler drains. Because strict flow control bounds
    /// the recv buffer to the granted window, this is also the per-stream buffer bound — a peer
    /// that sends past the window earns a connection-level `FLOW_CONTROL_ERROR`.
    ///
    /// Must be `>= h2_initial_stream_window_size`; a smaller value is clamped up to the initial
    /// (with a one-time log warning), since the window is only ever promoted upward.
    ///
    /// **Default**: `1 MiB` in bytes
    ///
    /// **Unit**: byte count
    pub(crate) h2_max_stream_recv_window_size: u32,

    /// Connection-level recv window target — how high the driver keeps the peer's
    /// connection-level window topped up as handlers consume bytes.
    ///
    /// Raised via an initial `WINDOW_UPDATE(stream_id=0)` right after SETTINGS (RFC 9113
    /// forbids SETTINGS from altering the connection window), then refilled on consumption.
    /// Bounds total concurrent in-flight request-body bytes across all streams on a single
    /// HTTP/2 connection. Leaving at the RFC baseline of `65_535` would cap bulk uploads at
    /// ~5 Mbit/s × RTT.
    ///
    /// **Default**: `2 MiB` in bytes
    ///
    /// **Unit**: byte count
    pub(crate) h2_initial_connection_window_size: u32,

    /// HTTP/2 `SETTINGS_MAX_CONCURRENT_STREAMS` — the maximum number of concurrent
    /// peer-initiated streams the server will accept.
    ///
    /// Peer-opened streams beyond this count get `RST_STREAM(RefusedStream)` per RFC 9113.
    /// A value in the 100–250 range is the post-Rapid-Reset (CVE-2023-44487) consensus;
    /// lower values cap parallelism, higher values need per-connection reset-rate limiting
    /// to avoid `DoS` exposure.
    ///
    /// **Default**: `100`
    ///
    /// **Unit**: stream count
    pub(crate) h2_max_concurrent_streams: u32,

    /// HTTP/2 `SETTINGS_MAX_FRAME_SIZE` — the largest frame payload the server will accept.
    ///
    /// Peer frames whose payload exceeds this get `FRAME_SIZE_ERROR` per RFC 9113. The RFC
    /// floor is `16_384`; the ceiling is `16_777_215`. Larger values amortize per-frame
    /// overhead on bulk transfers but increase the upper bound on a single read.
    ///
    /// **Default**: `16 KiB` in bytes
    ///
    /// **Unit**: byte count
    pub(crate) h2_max_frame_size: u32,

    /// whether [datagrams](https://www.rfc-editor.org/rfc/rfc9297.html) are enabled for HTTP/3
    ///
    /// This is a protocol-level setting and is communicated to the peer as well as enforced.
    ///
    /// **Default**: false
    pub(crate) h3_datagrams_enabled: bool,

    /// whether [webtransport](https://datatracker.ietf.org/doc/html/draft-ietf-webtrans-http3)
    /// (`draft-ietf-webtrans-http3`) is enabled for HTTP/3
    ///
    /// This is a protocol-level setting and is communicated to the peer. You do not need to
    /// manually configure this if using
    /// [`trillium-webtransport`](https://docs.rs/trillium-webtransport)
    ///
    /// **Default**: false
    pub(crate) webtransport_enabled: bool,

    /// `SETTINGS_ENABLE_CONNECT_PROTOCOL` — advertises that the server accepts extended
    /// CONNECT requests, enabling protocols layered on top of HTTP that bootstrap via a
    /// CONNECT with a `:protocol` pseudo-header.
    ///
    /// You likely don't need to set this directly if using a trillium handler that uses extended
    /// connect.
    ///
    /// **Default**: false
    pub(crate) extended_connect_enabled: bool,

    /// whether to panic when an outbound (app-controlled) header with an invalid value (containing
    /// `\r`, `\n`, or `\0`) is encountered.
    ///
    /// Invalid header values are always skipped to prevent header injection. When this is `true`,
    /// Trillium will additionally panic, surfacing the bug loudly. When `false`, the skip is only
    /// logged (to the `log` backend) at error level.
    ///
    /// **Default**: `true` when compiled with `debug_assertions` (i.e. debug builds), `false` in
    /// release builds. Override to `true` in release if you want strict production behavior, or to
    /// `false` in debug if you prefer not to panic during development.
    pub(crate) panic_on_invalid_response_headers: bool,
}

const KB: u32 = 1024;
const MB: u32 = 1024 * KB;

impl HttpConfig {
    /// Default Config
    pub const DEFAULT: Self = HttpConfig {
        response_buffer_len: 512,
        response_buffer_max_len: 2 * MB as usize,
        request_buffer_initial_len: 1024,
        head_max_len: 8 * KB as usize,
        response_header_initial_capacity: 32,
        request_header_initial_capacity: 32,
        copy_loops_per_yield: 16,
        received_body_max_len: 10 * MB as u64,
        received_body_initial_len: 128,
        received_body_max_preallocate: MB as usize,
        max_header_list_size: 32 * KB as u64,
        dynamic_table_capacity: 4 * KB as usize,
        h3_blocked_streams: 100,
        recent_pairs_size: 64,
        h3_datagrams_enabled: false,
        h2_initial_stream_window_size: 256 * KB,
        h2_max_stream_recv_window_size: MB,
        h2_initial_connection_window_size: 2 * MB,
        h2_max_concurrent_streams: 100,
        h2_max_frame_size: 16 * KB,
        webtransport_enabled: false,
        extended_connect_enabled: false,
        panic_on_invalid_response_headers: cfg!(debug_assertions),
    };
}

impl Default for HttpConfig {
    fn default() -> Self {
        HttpConfig::DEFAULT
    }
}
