//! Integration tests for HTTP Range request handling in
//! `trillium-static-compiled`.
//!
//! Range support is on by default and requires no cargo feature, so these
//! tests run unconditionally.

use etag::EntityTag;
use trillium_static_compiled::static_compiled;
use trillium_testing::{TestServer, block_on};

const LOREM_LEN: usize = 1147; // size of tests/fixtures/compressible/lorem.html

fn lorem_source() -> String {
    std::fs::read_to_string("./tests/fixtures/compressible/lorem.html").unwrap()
}

#[test]
fn accept_ranges_advertised_on_full_response() {
    block_on(async {
        let app = TestServer::new(static_compiled!("./tests/fixtures/compressible")).await;
        app.get("/lorem.html")
            .await
            .assert_ok()
            .assert_header("accept-ranges", "bytes");
    });
}

#[test]
fn simple_range_returns_206_with_slice() {
    let src = lorem_source();
    block_on(async {
        let app = TestServer::new(static_compiled!("./tests/fixtures/compressible")).await;
        app.get("/lorem.html")
            .with_request_header("range", "bytes=0-99")
            .await
            .assert_status(206)
            .assert_header("content-range", &*format!("bytes 0-99/{LOREM_LEN}"))
            .assert_header("accept-ranges", "bytes")
            .assert_body(&src[..=99]);
    });
}

#[test]
fn suffix_range_returns_last_n_bytes() {
    let src = lorem_source();
    block_on(async {
        let app = TestServer::new(static_compiled!("./tests/fixtures/compressible")).await;
        app.get("/lorem.html")
            .with_request_header("range", "bytes=-50")
            .await
            .assert_status(206)
            .assert_header(
                "content-range",
                &*format!("bytes {}-{}/{LOREM_LEN}", LOREM_LEN - 50, LOREM_LEN - 1),
            )
            .assert_body(&src[src.len() - 50..]);
    });
}

#[test]
fn open_ended_range_returns_to_end() {
    let src = lorem_source();
    block_on(async {
        let app = TestServer::new(static_compiled!("./tests/fixtures/compressible")).await;
        app.get("/lorem.html")
            .with_request_header("range", "bytes=1000-")
            .await
            .assert_status(206)
            .assert_header(
                "content-range",
                &*format!("bytes 1000-{}/{LOREM_LEN}", LOREM_LEN - 1),
            )
            .assert_body(&src[1000..]);
    });
}

#[test]
fn end_past_total_clamps_to_size_minus_one() {
    let src = lorem_source();
    block_on(async {
        let app = TestServer::new(static_compiled!("./tests/fixtures/compressible")).await;
        app.get("/lorem.html")
            .with_request_header("range", "bytes=0-99999")
            .await
            .assert_status(206)
            .assert_header(
                "content-range",
                &*format!("bytes 0-{}/{LOREM_LEN}", LOREM_LEN - 1),
            )
            .assert_body(&src);
    });
}

#[test]
fn out_of_bounds_returns_416() {
    block_on(async {
        let app = TestServer::new(static_compiled!("./tests/fixtures/compressible")).await;
        app.get("/lorem.html")
            .with_request_header("range", "bytes=99999-")
            .await
            .assert_status(416)
            .assert_header("content-range", &*format!("bytes */{LOREM_LEN}"))
            .assert_body("");
    });
}

#[test]
fn multi_range_falls_through_to_full_body() {
    let src = lorem_source();
    block_on(async {
        let app = TestServer::new(static_compiled!("./tests/fixtures/compressible")).await;
        app.get("/lorem.html")
            .with_request_header("range", "bytes=0-100,200-300")
            .await
            .assert_ok()
            .assert_no_header("content-range")
            .assert_body(&src);
    });
}

#[test]
fn invalid_range_header_falls_through_to_full_body() {
    let src = lorem_source();
    block_on(async {
        let app = TestServer::new(static_compiled!("./tests/fixtures/compressible")).await;
        app.get("/lorem.html")
            .with_request_header("range", "seconds=0-10")
            .await
            .assert_ok()
            .assert_no_header("content-range")
            .assert_body(&src);
    });
}

#[test]
fn if_range_matching_etag_honors_range() {
    let etag = EntityTag::from_data(lorem_source().as_bytes()).to_string();
    let src = lorem_source();
    block_on(async {
        let app = TestServer::new(static_compiled!("./tests/fixtures/compressible")).await;
        app.get("/lorem.html")
            .with_request_header("range", "bytes=0-49")
            .with_request_header("if-range", etag)
            .await
            .assert_status(206)
            .assert_body(&src[..=49]);
    });
}

#[test]
fn if_range_nonmatching_falls_through_to_full_body() {
    let src = lorem_source();
    block_on(async {
        let app = TestServer::new(static_compiled!("./tests/fixtures/compressible")).await;
        app.get("/lorem.html")
            .with_request_header("range", "bytes=0-49")
            .with_request_header("if-range", "\"some-other-etag\"")
            .await
            .assert_ok()
            .assert_no_header("content-range")
            .assert_body(&src);
    });
}

#[test]
fn range_bypasses_accept_encoding_negotiation() {
    #![cfg(feature = "compression")]
    let src = lorem_source();
    block_on(async {
        let app =
            TestServer::new(static_compiled!("./tests/fixtures/compressible", compress)).await;
        // Client accepts brotli AND requests a range. Range wins; identity
        // bytes are sliced; no Content-Encoding set.
        app.get("/lorem.html")
            .with_request_header("range", "bytes=0-99")
            .with_request_header("accept-encoding", "br")
            .await
            .assert_status(206)
            .assert_header("content-range", &*format!("bytes 0-99/{LOREM_LEN}"))
            .assert_no_header("content-encoding")
            .assert_body(&src[..=99]);
    });
}

#[test]
fn no_range_keeps_compression_behavior() {
    #![cfg(feature = "compression")]
    block_on(async {
        let app =
            TestServer::new(static_compiled!("./tests/fixtures/compressible", compress)).await;
        // No Range header; brotli still selected as before.
        app.get("/lorem.html")
            .with_request_header("accept-encoding", "br")
            .await
            .assert_ok()
            .assert_header("content-encoding", "br");
    });
}
