//! Microbenchmark for the HTTP/1.x head-parse path as a function of `request_buffer_initial_len`.
//!
//! Measures parse CPU + request-buffer growth in isolation: the transport yields all bytes on the
//! first poll and never parks, so the timed region is dominated by `Buffer::expand` reallocs and
//! header parsing rather than IO scheduling. Sweeping the initial buffer length against fixed,
//! representative request heads exposes the realloc penalty of undershooting a typical head size.
//!
//! Run with `cargo bench -p trillium-http --bench h1_head_parse`.

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use futures_lite::{AsyncRead, AsyncWrite, future::block_on};
use std::{
    hint::black_box,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use trillium_http::{HttpConfig, HttpContext, parse_head_for_bench};

/// In-memory transport that yields its bytes on read and discards writes. Reads always return
/// `Ready`, so driving the head parser over it incurs no task parking.
struct SliceTransport {
    data: Vec<u8>,
    pos: usize,
}

impl SliceTransport {
    fn new(data: &[u8]) -> Self {
        Self {
            data: data.to_vec(),
            pos: 0,
        }
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

fn assemble(request_line: &str, headers: &[(&str, &str)]) -> Vec<u8> {
    let mut s = String::from(request_line);
    s.push_str("\r\n");
    for (name, value) in headers {
        s.push_str(name);
        s.push_str(": ");
        s.push_str(value);
        s.push_str("\r\n");
    }
    s.push_str("\r\n");
    s.into_bytes()
}

/// A representative top-level-navigation GET, captured from Chrome 148 (the h2 pseudo-headers
/// translated to an h1 request line + `Host`, the three `Cookie` lines folded into one h1-legal
/// header, cookie values replaced with same-shape synthetic data).
fn browser_get() -> Vec<u8> {
    assemble(
        "GET / HTTP/1.1",
        &[
            ("Host", "quad.tanuki-sunfish.ts.net"),
            (
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,\
                 image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
            ),
            ("Accept-Encoding", "gzip, deflate, br, zstd"),
            ("Accept-Language", "en-US,en;q=0.9"),
            ("Cache-Control", "no-cache"),
            (
                "Cookie",
                "_octo=GH1.1.100000000.1700000000; \
                 datadome=AbCdEf01_2GhIjKl3mNoPqRs4tUvWxYz5AbCdEf06_7GhIjKl8mNoPqRs9tUvWxYz0\
                 AbCdEf12_3GhIjKl4mNoPqRs5tUvWxYz6AbCdEf78_9GhIjKlmNoPqRst; \
                 _dd_s=aid=00000000-0000-4000-8000-000000000000&rum=0&expire=1700000000000",
            ),
            ("DNT", "1"),
            ("Downlink", "10"),
            ("Pragma", "no-cache"),
            ("Priority", "u=0, i"),
            ("RTT", "50"),
            (
                "Sec-CH-UA",
                "\"Chromium\";v=\"148\", \"Google Chrome\";v=\"148\", \"Not/A)Brand\";v=\"99\"",
            ),
            ("Sec-CH-UA-Mobile", "?0"),
            ("Sec-CH-UA-Platform", "\"macOS\""),
            ("Sec-Fetch-Dest", "document"),
            ("Sec-Fetch-Mode", "navigate"),
            ("Sec-Fetch-Site", "none"),
            ("Sec-Fetch-User", "?1"),
            ("Sec-GPC", "1"),
            ("Upgrade-Insecure-Requests", "1"),
            (
                "User-Agent",
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36",
            ),
            ("sec-ch-ua-model", "\"\""),
            ("sec-ch-ua-full-version", "\"148.0.7778.179\""),
            ("sec-ch-prefers-color-scheme", "light"),
            (
                "sec-ch-ua-full-version-list",
                "\"Chromium\";v=\"148.0.7778.179\", \"Google Chrome\";v=\"148.0.7778.179\", \
                 \"Not/A)Brand\";v=\"99.0.0.0\"",
            ),
            ("sec-ch-ua-wow64", "?0"),
            ("sec-ch-ua-platform-version", "\"15.7.7\""),
            ("sec-ch-ua-form-factors", "\"Desktop\""),
            ("sec-ch-ua-bitness", "\"64\""),
            ("sec-ch-ua-arch", "\"x86\""),
        ],
    )
}

/// A typical JSON API request: bearer auth, content negotiation, a small fixed-length body
/// (head only — the parser stops at the terminator).
fn api_post() -> Vec<u8> {
    assemble(
        "POST /v1/widgets HTTP/1.1",
        &[
            ("Host", "api.example.com"),
            ("Accept", "application/json"),
            ("Content-Type", "application/json"),
            ("Content-Length", "82"),
            (
                "Authorization",
                "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.\
                 dummysignaturedummysignaturedummysig",
            ),
            ("User-Agent", "acme-client/2.4.1"),
            ("X-Request-Id", "7f3a9c1e-2b4d-4a6f-8e1c-9d0b5a3f2c11"),
        ],
    )
}

/// A header-heavy proxy passthrough: forwarding metadata, tracing, and an accumulated cookie jar.
fn proxy_heavy() -> Vec<u8> {
    assemble(
        "GET /account/settings HTTP/1.1",
        &[
            ("Host", "app.example.com"),
            ("Accept", "*/*"),
            ("Accept-Encoding", "gzip, deflate, br"),
            ("Accept-Language", "en-US,en;q=0.9"),
            ("X-Forwarded-For", "203.0.113.7, 198.51.100.42, 10.0.0.3"),
            ("X-Forwarded-Proto", "https"),
            ("X-Forwarded-Host", "app.example.com"),
            ("X-Forwarded-Port", "443"),
            ("X-Real-IP", "203.0.113.7"),
            (
                "Forwarded",
                "for=203.0.113.7;proto=https;host=app.example.com, for=198.51.100.42",
            ),
            ("Via", "1.1 edge-cache-07 (squid/5.7), 1.1 lb-02"),
            ("X-Request-Id", "b1946ac9-2f4c-4a1d-9c3e-7e2a8f5d0c44"),
            ("X-Trace-Id", "0af7651916cd43dd8448eb211c80319c"),
            ("X-B3-TraceId", "80f198ee56343ba864fe8b2a57d3eff7"),
            ("X-B3-SpanId", "e457b5a2e4d86bd1"),
            ("X-B3-Sampled", "1"),
            ("CF-Connecting-IP", "203.0.113.7"),
            ("CF-Ray", "8a1b2c3d4e5f6789-SJC"),
            ("CF-IPCountry", "US"),
            (
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/147.0.0.0 Safari/537.36",
            ),
            (
                "Cookie",
                "session=a1b2c3d4e5f60718293a4b5c6d7e8f90; \
                 csrftoken=ZmFrZS1jc3JmLXRva2VuLXZhbHVlLTEyMzQ1Njc4OTA; \
                 _ga=GA1.2.100000000.1700000000; _gid=GA1.2.200000000.1700000000; \
                 ajs_user_id=u_0000000000; ajs_anonymous_id=00000000-0000-4000-8000-000000000000",
            ),
            ("Referer", "https://app.example.com/dashboard"),
        ],
    )
}

fn bench_parse(c: &mut Criterion) {
    let anchors = [
        ("browser_get", browser_get()),
        ("api_post", api_post()),
        ("proxy_heavy", proxy_heavy()),
    ];
    let initial_lens = [128usize, 256, 512, 1024, 2048, 4096];

    for (name, bytes) in &anchors {
        let mut group = c.benchmark_group(format!("h1_head_parse/{name}"));
        group.throughput(Throughput::Bytes(bytes.len() as u64));
        for &initial_len in &initial_lens {
            let ctx = Arc::new(
                HttpContext::new()
                    .with_config(HttpConfig::DEFAULT.with_request_buffer_initial_len(initial_len)),
            );
            group.bench_with_input(
                BenchmarkId::from_parameter(initial_len),
                &initial_len,
                |b, _| {
                    b.iter_batched(
                        || SliceTransport::new(bytes),
                        |transport| {
                            black_box(
                                block_on(parse_head_for_bench(ctx.clone(), transport)).unwrap(),
                            );
                        },
                        BatchSize::SmallInput,
                    );
                },
            );
        }
        group.finish();
    }
}

criterion_group!(benches, bench_parse);
criterion_main!(benches);
