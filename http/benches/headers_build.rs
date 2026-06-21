//! Microbenchmark for `Headers` construction from a parsed header block, isolating the
//! known/unknown store-sizing tradeoff.
//!
//! `Headers` keeps two maps — known headers (enum-keyed) and unknown headers (string-keyed) — and
//! `with_capacity` historically sized only the known store. This sweep seeds both stores at various
//! capacities and reparses two representative header blocks (a known-heavy browser navigation and
//! an unknown-heavy proxy passthrough) to measure how much the from-zero growth of each store
//! costs, and whether a single split capacity captures the benefit of sizing both.
//!
//! Run with `cargo bench -p trillium-http --bench headers_build`.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use trillium_http::Headers;

fn block(headers: &[(&str, &str)]) -> Vec<u8> {
    let mut s = String::new();
    for (name, value) in headers {
        s.push_str(name);
        s.push_str(": ");
        s.push_str(value);
        s.push_str("\r\n");
    }
    s.into_bytes()
}

/// Known-heavy: most field names are `KnownHeaderName` variants (incl. the standardized client
/// hints / fetch-metadata headers); only the newer `sec-ch-ua-*` hints land in the unknown store.
fn browser_block() -> Vec<u8> {
    block(&[
        ("Host", "quad.tanuki-sunfish.ts.net"),
        (
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        ),
        ("Accept-Encoding", "gzip, deflate, br, zstd"),
        ("Accept-Language", "en-US,en;q=0.9"),
        ("Cache-Control", "no-cache"),
        (
            "Cookie",
            "_octo=GH1.1.100000000.1700000000; _dd_s=rum=0&expire=1700000000000",
        ),
        ("DNT", "1"),
        ("Downlink", "10"),
        ("Pragma", "no-cache"),
        ("Priority", "u=0, i"),
        ("RTT", "50"),
        (
            "Sec-CH-UA",
            "\"Chromium\";v=\"148\", \"Not/A)Brand\";v=\"99\"",
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
            "Mozilla/5.0 (Macintosh) Chrome/148.0.0.0 Safari/537.36",
        ),
        ("sec-ch-ua-model", "\"\""),
        ("sec-ch-ua-full-version", "\"148.0.7778.179\""),
        ("sec-ch-prefers-color-scheme", "light"),
        ("sec-ch-ua-wow64", "?0"),
        ("sec-ch-ua-platform-version", "\"15.7.7\""),
        ("sec-ch-ua-form-factors", "\"Desktop\""),
        ("sec-ch-ua-bitness", "\"64\""),
        ("sec-ch-ua-arch", "\"x86\""),
    ])
}

/// Unknown-heavy: forwarding/tracing/CDN field names are mostly not `KnownHeaderName` variants, so
/// they pile into the unknown store.
fn proxy_block() -> Vec<u8> {
    block(&[
        ("Host", "app.example.com"),
        ("Accept", "*/*"),
        ("Accept-Encoding", "gzip, deflate, br"),
        ("X-Forwarded-For", "203.0.113.7, 198.51.100.42, 10.0.0.3"),
        ("X-Forwarded-Proto", "https"),
        ("X-Forwarded-Host", "app.example.com"),
        ("X-Forwarded-Port", "443"),
        ("X-Real-IP", "203.0.113.7"),
        (
            "Forwarded",
            "for=203.0.113.7;proto=https;host=app.example.com",
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
            "Mozilla/5.0 (Windows NT 10.0) Chrome/147.0.0.0 Safari/537.36",
        ),
        ("X-Tenant-Id", "tenant_000000"),
        ("X-Api-Version", "2024-11-01"),
        ("Referer", "https://app.example.com/dashboard"),
    ])
}

fn bench_build(c: &mut Criterion) {
    // (label, known cap, unknown cap)
    let seedings = [
        ("16/0_default", 16usize, 0usize),
        ("32/0_known", 32, 0),
        ("16/16_split32", 16, 16),
        ("24/24_split48", 24, 24),
        ("64/64_floor", 64, 64),
    ];

    for (profile, bytes) in [("browser", browser_block()), ("proxy", proxy_block())] {
        let mut group = c.benchmark_group(format!("headers_build/{profile}"));
        for (label, known, unknown) in seedings {
            group.bench_with_input(BenchmarkId::from_parameter(label), &label, |b, _| {
                b.iter(|| {
                    let mut headers = Headers::with_capacities(known, unknown);
                    headers.extend_parse(black_box(&bytes)).unwrap();
                    black_box(&headers);
                });
            });
        }
        group.finish();
    }
}

criterion_group!(benches, bench_build);
criterion_main!(benches);
