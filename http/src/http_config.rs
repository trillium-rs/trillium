#![allow(dead_code)]

use fieldwork::Fieldwork;

pub const DEFAULT_CONFIG: HttpConfig = HttpConfig {
    response_buffer_len: 512,
    request_buffer_initial_len: 128,
    head_max_len: 8 * 1024,
    max_headers: 128,
    response_header_initial_capacity: 16,
    copy_loops_per_yield: 16,
    received_body_max_len: 500 * 1024 * 1024,
    received_body_initial_len: 128,
    received_body_max_preallocate: 1024 * 1024,
    h3_max_field_section_size: 8 * 1024,
    h3_datagrams_enabled: false,
    webtransport_enabled: false,
    panic_on_invalid_response_headers: cfg!(debug_assertions),
};

/// # Performance and security parameters for trillium-http.
///
/// Trillium's http implementation is built with sensible defaults, but applications differ in usage
/// and this escape hatch allows an application to be tuned. It is best to tune these parameters in
/// context of realistic benchmarks for your application.
///
/// Long term, trillium may export several standard defaults for different constraints and
/// application types. In the distant future, these may turn into initial values and trillium will
/// tune itself based on values seen at runtime.
#[derive(Clone, Copy, Debug, Fieldwork)]
#[fieldwork(get, get_mut, set, with, without)]
pub struct HttpConfig {
    /// The maximum length allowed before the http body begins for a given request.
    ///
    /// **Default**: `8kb` in bytes
    ///
    /// **Unit**: Byte count
    pub(crate) head_max_len: usize,

    /// The maximum length of a received body
    ///
    /// This applies to both chunked and fixed-length request bodies, and the correct value will be
    /// application dependent.
    ///
    /// **Default**: `500mb` in bytes
    ///
    /// **Unit**: Byte count
    pub(crate) received_body_max_len: u64,

    #[field = false] // this one is private for now
    pub(crate) max_headers: usize,

    /// The initial buffer allocated for the response.
    ///
    /// Ideally this would be exactly the length of the combined response headers and body, if the
    /// body is short. If the value is shorter than the headers plus the body, multiple transport
    /// writes will be performed, and if the value is longer, unnecessary memory will be allocated
    /// for each conn. Although a tcp packet can be up to 64kb, it is probably better to use a
    /// value less than 1.5kb.
    ///
    /// **Default**: `512`
    ///
    /// **Unit**: byte count
    pub(crate) response_buffer_len: usize,

    /// The initial buffer allocated for the request headers.
    ///
    /// Ideally this is the length of the request headers. It will grow nonlinearly until
    /// `max_head_len` or the end of the headers are reached, whichever happens first.
    ///
    /// **Default**: `128`
    ///
    /// **Unit**: byte count
    pub(crate) request_buffer_initial_len: usize,

    /// The number of response headers to allocate space for on conn creation.
    ///
    /// Headers will grow on insertion when they reach this size.
    ///
    /// **Default**: `16`
    ///
    /// **Unit**: Header count
    pub(crate) response_header_initial_capacity: usize,

    /// A sort of cooperative task yielding knob.
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

    /// The maximum size of a field section (header block) the peer may send in HTTP/3
    ///
    /// This is a protocol-level setting and is communicated to the peer.
    ///
    /// **Default**: 8kb
    ///
    /// **Unit**: Byte count
    pub(crate) h3_max_field_section_size: u64,

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

    /// whether to panic when a response header with an invalid value (containing `\r`, `\n`, or
    /// `\0`) is encountered.
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

impl Default for HttpConfig {
    fn default() -> Self {
        DEFAULT_CONFIG
    }
}
