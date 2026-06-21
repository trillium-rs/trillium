//! Counts the number of transport writes (≈ `writev` syscalls) the HTTP/1.x send path performs for
//! a response, as a function of `response_buffer_len` and response-body size.
//!
//! Drives the real `HttpContext::run` send path over an in-memory transport that tallies every
//! `poll_write`/`poll_write_vectored` rather than measuring time — the per-syscall cost is a known
//! ~µs constant, so the write *count* is the quantity that decides the knob. The transport accepts
//! every write in full, so one tally == one syscall the kernel would have seen.
//!
//! Run with `cargo run -p trillium-http --example response_writes --release`.

use futures_lite::{AsyncRead, AsyncWrite, future::block_on};
use std::{
    io::{self, IoSlice},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};
use trillium_http::{Conn, HttpConfig, HttpContext};

#[derive(Clone, Default)]
struct Counters {
    writes: Arc<AtomicUsize>,
    bytes: Arc<AtomicUsize>,
}

/// Serves a single request on read, then EOF; tallies each write and the bytes it carried.
struct CountingTransport {
    request: Vec<u8>,
    read_pos: usize,
    counters: Counters,
}

impl CountingTransport {
    fn new(request: &[u8], counters: Counters) -> Self {
        Self {
            request: request.to_vec(),
            read_pos: 0,
            counters,
        }
    }
}

impl AsyncRead for CountingTransport {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let remaining = &this.request[this.read_pos..];
        let n = remaining.len().min(buf.len());
        buf[..n].copy_from_slice(&remaining[..n]);
        this.read_pos += n;
        Poll::Ready(Ok(n))
    }
}

impl AsyncWrite for CountingTransport {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.counters.writes.fetch_add(1, Ordering::Relaxed);
        this.counters.bytes.fetch_add(buf.len(), Ordering::Relaxed);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let total: usize = bufs.iter().map(|b| b.len()).sum();
        this.counters.writes.fetch_add(1, Ordering::Relaxed);
        this.counters.bytes.fetch_add(total, Ordering::Relaxed);
        Poll::Ready(Ok(total))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn write_count(response_buffer_len: usize, body_size: usize) -> (usize, usize) {
    let counters = Counters::default();
    let ctx = Arc::new(
        HttpContext::new()
            .with_config(HttpConfig::DEFAULT.with_response_buffer_len(response_buffer_len)),
    );
    let transport = CountingTransport::new(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n", counters.clone());

    block_on(
        ctx.run(transport, move |conn: Conn<CountingTransport>| async move {
            conn.with_response_body(vec![0u8; body_size])
                .with_status(200)
        }),
    )
    .unwrap();

    (
        counters.writes.load(Ordering::Relaxed),
        counters.bytes.load(Ordering::Relaxed),
    )
}

fn main() {
    let buffer_lens = [512usize, 2048, 8192, 16384, 65536];
    let body_sizes = [0usize, 256, 1024, 4096, 16384, 65536, 262_144];

    print!("{:>14}", "body \\ buf");
    for buf in buffer_lens {
        print!("{buf:>10}");
    }
    println!();

    for body in body_sizes {
        print!("{body:>14}");
        for buf in buffer_lens {
            let (writes, _bytes) = write_count(buf, body);
            print!("{writes:>10}");
        }
        println!();
    }
    println!(
        "\n(cells = transport write count for one response; rows = body bytes, cols = \
         response_buffer_len)"
    );
}
