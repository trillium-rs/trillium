//! Integration tests for compile-time etag computation.
//!
//! Etag generation is on by default and requires no cargo feature, so these
//! tests run unconditionally.

use etag::EntityTag;
use trillium_caching_headers::Etag;
use trillium_static_compiled::static_compiled;
use trillium_testing::{TestServer, block_on};

/// Precomputed source bytes of the largest fixture file. Kept in sync by
/// reading at test time so an inadvertent fixture edit doesn't make the test
/// silently stale.
fn lorem_source() -> Vec<u8> {
    std::fs::read("./tests/fixtures/compressible/lorem.html").expect("fixture file must exist")
}

#[test]
fn etag_present_by_default() {
    let expected = EntityTag::from_data(&lorem_source()).to_string();
    block_on(async {
        let app = TestServer::new(static_compiled!("./tests/fixtures/compressible")).await;
        app.get("/lorem.html")
            .await
            .assert_ok()
            .assert_header("etag", &*expected);
    });
}

#[test]
fn etag_false_suppresses_header() {
    block_on(async {
        let app = TestServer::new(static_compiled!(
            "./tests/fixtures/compressible",
            etag = false
        ))
        .await;
        app.get("/lorem.html")
            .await
            .assert_ok()
            .assert_no_header("etag");
    });
}

#[test]
fn etag_true_is_explicit_default() {
    let expected = EntityTag::from_data(&lorem_source()).to_string();
    block_on(async {
        let app = TestServer::new(static_compiled!(
            "./tests/fixtures/compressible",
            etag = true
        ))
        .await;
        app.get("/lorem.html")
            .await
            .assert_ok()
            .assert_header("etag", &*expected);
    });
}

#[test]
fn precomputed_etag_matches_caching_headers_runtime() {
    // The caching-headers Etag handler computes etags from response bodies
    // using the same `etag::EntityTag::from_data`. Chain it after the
    // static-compiled handler — it should see the precomputed etag and skip
    // its own computation, leaving the header unchanged.
    let expected = EntityTag::from_data(&lorem_source()).to_string();
    block_on(async {
        let app = TestServer::new((
            static_compiled!("./tests/fixtures/compressible"),
            Etag::new(),
        ))
        .await;
        app.get("/lorem.html")
            .await
            .assert_ok()
            .assert_header("etag", &*expected);
    });
}

#[test]
fn matching_if_none_match_returns_304_with_etag_handler() {
    let expected = EntityTag::from_data(&lorem_source()).to_string();
    block_on(async {
        let app = TestServer::new((
            static_compiled!("./tests/fixtures/compressible"),
            Etag::new(),
        ))
        .await;
        app.get("/lorem.html")
            .with_request_header("if-none-match", expected)
            .await
            .assert_status(304);
    });
}

#[test]
fn nonmatching_if_none_match_returns_200() {
    block_on(async {
        let app = TestServer::new((
            static_compiled!("./tests/fixtures/compressible"),
            Etag::new(),
        ))
        .await;
        app.get("/lorem.html")
            .with_request_header("if-none-match", "\"some-other-etag\"")
            .await
            .assert_ok();
    });
}

#[test]
fn tiny_files_also_get_etags() {
    // Etag baking is independent of the compression min-size threshold.
    let src = std::fs::read("./tests/fixtures/compressible/tiny.txt").unwrap();
    let expected = EntityTag::from_data(&src).to_string();
    block_on(async {
        let app = TestServer::new(static_compiled!("./tests/fixtures/compressible")).await;
        app.get("/tiny.txt")
            .await
            .assert_ok()
            .assert_header("etag", &*expected);
    });
}

#[test]
fn etag_present_alongside_compression() {
    #![cfg(feature = "compression")]
    let expected = EntityTag::from_data(&lorem_source()).to_string();
    block_on(async {
        let app =
            TestServer::new(static_compiled!("./tests/fixtures/compressible", compress)).await;
        // Brotli variant: same etag as identity (one etag per source).
        app.get("/lorem.html")
            .with_request_header("accept-encoding", "br")
            .await
            .assert_ok()
            .assert_header("content-encoding", "br")
            .assert_header("etag", &*expected);
        // Identity: same etag.
        app.get("/lorem.html")
            .await
            .assert_ok()
            .assert_no_header("content-encoding")
            .assert_header("etag", &*expected);
    });
}

#[test]
fn no_etag_with_compression() {
    #![cfg(feature = "compression")]
    block_on(async {
        let app = TestServer::new(static_compiled!(
            "./tests/fixtures/compressible",
            compress,
            etag = false
        ))
        .await;
        app.get("/lorem.html")
            .with_request_header("accept-encoding", "br")
            .await
            .assert_ok()
            .assert_header("content-encoding", "br")
            .assert_no_header("etag");
    });
}
