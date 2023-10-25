#![allow(dead_code)]

#[derive(Clone, Copy, Debug)]
pub struct HttpConfig {
    pub(crate) write_buffer_len: usize,
    pub(crate) read_buffer_len: usize,
    pub(crate) max_head_len: usize,
    pub(crate) max_headers: usize,
    pub(crate) initial_header_capacity: usize,
    pub(crate) copy_loops_per_yield: usize,
    pub(crate) received_body_max_len: u64,
    pub(crate) received_body_initial_len: usize,
}

impl HttpConfig {
    pub(crate) fn with_write_buffer_len(mut self, write_buffer_len: usize) -> Self {
        self.write_buffer_len = write_buffer_len;
        self
    }

    pub(crate) fn with_read_buffer_len(mut self, read_buffer_len: usize) -> Self {
        self.read_buffer_len = read_buffer_len;
        self
    }

    pub(crate) fn with_max_head_len(mut self, max_head_len: usize) -> Self {
        self.max_head_len = max_head_len;
        self
    }

    pub(crate) fn with_max_headers(mut self, max_headers: usize) -> Self {
        self.max_headers = max_headers;
        self
    }

    pub(crate) fn with_initial_header_capacity(mut self, initial_header_capacity: usize) -> Self {
        self.initial_header_capacity = initial_header_capacity;
        self
    }

    pub(crate) fn with_copy_loops_per_yield(mut self, copy_loops_per_yield: usize) -> Self {
        self.copy_loops_per_yield = copy_loops_per_yield;
        self
    }

    pub(crate) fn with_received_body_max_len(mut self, received_body_max_len: u64) -> Self {
        self.received_body_max_len = received_body_max_len;
        self
    }
}

impl Default for HttpConfig {
    fn default() -> Self {
        DEFAULT_CONFIG
    }
}

pub const DEFAULT_CONFIG: HttpConfig = HttpConfig {
    write_buffer_len: 512,
    read_buffer_len: 128,
    max_head_len: 8 * 1024,
    max_headers: 128,
    initial_header_capacity: 16,
    copy_loops_per_yield: 16,
    received_body_max_len: 524_288_000u64,
    received_body_initial_len: 128,
};
