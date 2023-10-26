#![allow(dead_code)]

pub const DEFAULT_CONFIG: HttpConfig = HttpConfig {
    response_buffer_len: 512,
    request_buffer_initial_len: 128,
    head_max_len: 4 * 1024,
    max_headers: 128,
    response_header_initial_capacity: 16,
    copy_loops_per_yield: 16,
    received_body_max_len: 524_288_000u64,
    received_body_initial_len: 128,
};

/**
# Performance and security parameters for trillium-http.

Trillium's http implementation is built with sensible defaults, but applications differ in usage and
this escape hatch allows an application to be tuned. It is best to tune these parameters in context
of realistic benchmarks for your application.

Long term, trillium may export several standard defaults for different constraints and application
types. In the distant future, these may turn into initial values and trillium will tune itself based
on values seen at runtime.


## Performance parameters

### `response_buffer_len`

The initial buffer allocated for the response. Ideally this would be exactly the length of the
combined response headers and body, if the body is short. If the value is shorter than the headers
plus the body, multiple transport writes will be performed, and if the value is longer, unnecessary
memory will be allocated for each conn. Although a tcp packet can be up to 64kb, it is probably
better to use a value less than 1.5kb.

**Default**: `512`

**Unit**: byte count

### `request_buffer_initial_len`

The initial buffer allocated for the request headers. Ideally this is the length of the request
headers. It will grow nonlinearly until `max_head_len` or the end of the headers are reached,
whichever happens first.

**Default**: `128`

**Unit**: byte count

### `received_body_initial_len`

The initial buffer capacity allocated when reading a chunked http body to bytes or string. Ideally
this would be the size of the http body, which is highly application dependent. As with other
initial buffer lengths, further allocation will be performed until the necessary length is
achieved. A smaller number will result in more vec resizing, and a larger number will result in
unnecessary allocation.

**Default**: `128`

**Unit**: byte count


### `copy_loops_per_yield`

A sort of cooperative task yielding knob. Decreasing this number will improve tail latencies at a
slight cost to total throughput for fast clients. This will have more of an impact on servers that
spend a lot of time in IO compared to app handlers.

**Default**: `16`

**Unit**: the number of consecutive `Poll::Ready` async writes to perform before yielding
the task back to the runtime.

### `response_header_initial_capacity`

The number of response headers to allocate space for on conn creation. Headers will grow on
insertion when they reach this size.

**Default**: `16`

**Unit**: Header count

## Security parameters

These parameters represent worst cases, to delineate between malicious (or malformed) requests and
acceptable ones.

### `head_max_len`

The maximum length allowed before the http body begins for a given request.

**Default**: `4kb` in bytes

**Unit**: Byte count

### `received_body_max_len`

The maximum length of a received body. This applies to both chunked and fixed-length request bodies,
and the correct value will be application dependent.

**Default**: `500mb` in bytes

**Unit**: Byte count

*/

#[derive(Clone, Copy, Debug)]
pub struct HttpConfig {
    pub(crate) head_max_len: usize,
    pub(crate) received_body_max_len: u64,
    pub(crate) max_headers: usize,
    pub(crate) response_buffer_len: usize,
    pub(crate) request_buffer_initial_len: usize,
    pub(crate) response_header_initial_capacity: usize,
    pub(crate) copy_loops_per_yield: usize,
    pub(crate) received_body_initial_len: usize,
}

#[allow(missing_docs)]
impl HttpConfig {
    /// See [`response_buffer_len`][HttpConfig#response_buffer_len]
    #[must_use]
    pub fn with_response_buffer_len(mut self, response_buffer_len: usize) -> Self {
        self.response_buffer_len = response_buffer_len;
        self
    }

    /// See [`request_buffer_initial_len`][HttpConfig#request_buffer_initial_len]
    #[must_use]
    pub fn with_request_buffer_initial_len(mut self, request_buffer_initial_len: usize) -> Self {
        self.request_buffer_initial_len = request_buffer_initial_len;
        self
    }

    /// See [`head_max_len`][HttpConfig#head_max_len]
    #[must_use]
    pub fn with_head_max_len(mut self, head_max_len: usize) -> Self {
        self.head_max_len = head_max_len;
        self
    }

    /// See [`response_header_initial_capacity`][HttpConfig#resopnse_header_initial_capacity]
    #[must_use]
    pub fn with_response_header_initial_capacity(
        mut self,
        response_header_initial_capacity: usize,
    ) -> Self {
        self.response_header_initial_capacity = response_header_initial_capacity;
        self
    }

    /// See [`copy_loops_per_yield`][HttpConfig#copy_loops_per_yield]
    #[must_use]
    pub fn with_copy_loops_per_yield(mut self, copy_loops_per_yield: usize) -> Self {
        self.copy_loops_per_yield = copy_loops_per_yield;
        self
    }

    /// See [`received_body_max_len`][HttpConfig#received_body_max_len]
    #[must_use]
    pub fn with_received_body_max_len(mut self, received_body_max_len: u64) -> Self {
        self.received_body_max_len = received_body_max_len;
        self
    }
}

impl Default for HttpConfig {
    fn default() -> Self {
        DEFAULT_CONFIG
    }
}
