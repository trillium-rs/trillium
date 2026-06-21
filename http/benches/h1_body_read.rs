//! Microbenchmarks for reading an HTTP/1.x request body to bytes, isolating the two inbound-body
//! sizing knobs:
//!
//! - `received_body_max_preallocate` — for a fixed-length (Content-Length) body, `read_bytes`
//!   preallocates `min(content_length, max_preallocate)`, then grows. Sweeping the cap above and
//!   below the body size exposes the realloc cost of *not* trusting the declared length enough to
//!   preallocate it (the question: how much does a low cap cost an honest large upload?).
//! - `received_body_initial_len` — for a chunked body (no declared length), the accumulator starts
//!   here and grows; sweeping it shows the realloc cost of undershooting.
//!
//! The transport yields all bytes on read and never parks, so the timed region is buffer growth +
//! byte copying, not IO scheduling — i.e. the in-memory *upper bound* on the realloc penalty; over
//! a real network the body's arrival cost dwarfs it.
//!
//! Run with `cargo bench -p trillium-http --bench h1_body_read`.

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use futures_lite::{AsyncRead, AsyncWrite, future::block_on};
use std::{
    hint::black_box,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use trillium_http::{Conn, HttpConfig, HttpContext, parse_head_for_bench};

/// In-memory transport: yields its bytes on read (never parks), discards writes.
struct SliceTransport {
    data: Arc<[u8]>,
    pos: usize,
}

impl SliceTransport {
    fn new(data: Arc<[u8]>) -> Self {
        Self { data, pos: 0 }
    }
}

impl AsyncRead for SliceTransport {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let remaining = &this.data[this.pos..];
        let n = remaining.len().min(buf.len());
        buf[..n].copy_from_slice(&remaining[..n]);
        this.pos += n;
        Poll::Ready(Ok(n))
    }
}

impl AsyncWrite for SliceTransport {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn fixed_length_request(body: usize) -> Arc<[u8]> {
    let mut v =
        format!("POST / HTTP/1.1\r\nHost: x\r\nContent-Length: {body}\r\n\r\n").into_bytes();
    v.extend(std::iter::repeat_n(b'a', body));
    v.into()
}

fn chunked_request(body: usize) -> Arc<[u8]> {
    let mut v = b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    v.extend_from_slice(format!("{body:x}\r\n").as_bytes());
    v.extend(std::iter::repeat_n(b'a', body));
    v.extend_from_slice(b"\r\n0\r\n\r\n");
    v.into()
}

fn context(config: HttpConfig) -> Arc<HttpContext> {
    // Lift the hard ceiling above every test body so it's the preallocate/initial knob under test,
    // not `received_body_max_len`, that governs.
    Arc::new(HttpContext::new().with_config(config.with_received_body_max_len(64 << 20)))
}

fn read_body(ctx: &Arc<HttpContext>, request: &Arc<[u8]>) -> usize {
    let mut conn: Conn<SliceTransport> = block_on(parse_head_for_bench(
        ctx.clone(),
        SliceTransport::new(request.clone()),
    ))
    .unwrap();
    block_on(conn.request_body().read_bytes()).unwrap().len()
}

fn bench_preallocate(c: &mut Criterion) {
    let caps = [64 << 10, 256 << 10, 1 << 20, 4 << 20, 10 << 20];
    for body in [2 << 20usize, 8 << 20] {
        let request = fixed_length_request(body);
        let mut group = c.benchmark_group(format!("body_read/preallocate/{}KiB", body >> 10));
        group.throughput(Throughput::Bytes(body as u64));
        for cap in caps {
            let ctx = context(HttpConfig::DEFAULT.with_received_body_max_preallocate(cap));
            group.bench_with_input(BenchmarkId::from_parameter(cap), &cap, |b, _| {
                b.iter_batched(
                    || request.clone(),
                    |req| black_box(read_body(&ctx, &req)),
                    BatchSize::SmallInput,
                );
            });
        }
        group.finish();
    }
}

fn bench_chunked_initial(c: &mut Criterion) {
    let initials = [128usize, 1 << 10, 8 << 10, 64 << 10, 1 << 20];
    for body in [64 << 10usize, 1 << 20] {
        let request = chunked_request(body);
        let mut group = c.benchmark_group(format!("body_read/chunked_initial/{}KiB", body >> 10));
        group.throughput(Throughput::Bytes(body as u64));
        for initial in initials {
            let ctx = context(HttpConfig::DEFAULT.with_received_body_initial_len(initial));
            group.bench_with_input(BenchmarkId::from_parameter(initial), &initial, |b, _| {
                b.iter_batched(
                    || request.clone(),
                    |req| black_box(read_body(&ctx, &req)),
                    BatchSize::SmallInput,
                );
            });
        }
        group.finish();
    }
}

criterion_group!(benches, bench_preallocate, bench_chunked_initial);
criterion_main!(benches);
