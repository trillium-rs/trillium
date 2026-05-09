//! Integration tests for compile-time precompression in
//! `trillium-static-compiled`.
//!
//! Gated on the `compression` meta-feature so that running `cargo test` with
//! default features doesn't require any encoder dependencies.

#![cfg(feature = "compression")]

use trillium::{Conn, KnownHeaderName::Vary};
use trillium_static_compiled::{Encoding, static_compiled};
use trillium_testing::{TestServer, block_on};

#[test]
fn brotli_returned_when_accepted() {
    block_on(async {
        let app =
            TestServer::new(static_compiled!("./tests/fixtures/compressible", compress)).await;
        app.get("/lorem.html")
            .with_request_header("accept-encoding", "br")
            .await
            .assert_ok()
            .assert_header("content-encoding", "br")
            .assert_header("vary", "Accept-Encoding")
            .assert_header("content-type", "text/html");
    });
}

#[test]
fn gzip_returned_when_only_gzip_accepted() {
    block_on(async {
        let app =
            TestServer::new(static_compiled!("./tests/fixtures/compressible", compress)).await;
        app.get("/lorem.html")
            .with_request_header("accept-encoding", "gzip")
            .await
            .assert_ok()
            .assert_header("content-encoding", "gzip")
            .assert_header("vary", "Accept-Encoding");
    });
}

#[test]
fn zstd_returned_when_only_zstd_accepted() {
    block_on(async {
        let app =
            TestServer::new(static_compiled!("./tests/fixtures/compressible", compress)).await;
        app.get("/lorem.html")
            .with_request_header("accept-encoding", "zstd")
            .await
            .assert_ok()
            .assert_header("content-encoding", "zstd")
            .assert_header("vary", "Accept-Encoding");
    });
}

#[test]
fn smallest_variant_wins_when_all_accepted() {
    // For natural-language text, brotli q=11 reliably beats zstd 22 and gzip 9.
    // Variants are sorted smallest-first at macro time, so brotli should win.
    block_on(async {
        let app =
            TestServer::new(static_compiled!("./tests/fixtures/compressible", compress)).await;
        app.get("/lorem.html")
            .with_request_header("accept-encoding", "br, zstd, gzip")
            .await
            .assert_ok()
            .assert_header("content-encoding", "br");
    });
}

#[test]
fn no_accept_encoding_serves_identity_with_vary() {
    block_on(async {
        let app =
            TestServer::new(static_compiled!("./tests/fixtures/compressible", compress)).await;
        app.get("/lorem.html")
            .await
            .assert_ok()
            .assert_no_header("content-encoding")
            .assert_header("vary", "Accept-Encoding");
    });
}

#[test]
fn unaccepted_encodings_fall_back_to_identity() {
    block_on(async {
        let app =
            TestServer::new(static_compiled!("./tests/fixtures/compressible", compress)).await;
        app.get("/lorem.html")
            .with_request_header("accept-encoding", "br;q=0, zstd;q=0, gzip;q=0")
            .await
            .assert_ok()
            .assert_no_header("content-encoding")
            .assert_header("vary", "Accept-Encoding");
    });
}

#[test]
fn wildcard_accepts_smallest_variant() {
    block_on(async {
        let app =
            TestServer::new(static_compiled!("./tests/fixtures/compressible", compress)).await;
        app.get("/lorem.html")
            .with_request_header("accept-encoding", "*")
            .await
            .assert_ok()
            .assert_header("content-encoding", "br");
    });
}

#[test]
fn tiny_file_below_threshold_is_not_compressed() {
    // tiny.txt is 11 bytes, far below the 256 byte threshold — no variants
    // baked, no Vary either.
    block_on(async {
        let app =
            TestServer::new(static_compiled!("./tests/fixtures/compressible", compress)).await;
        app.get("/tiny.txt")
            .with_request_header("accept-encoding", "br, zstd, gzip")
            .await
            .assert_ok()
            .assert_no_header("content-encoding")
            .assert_no_header("vary");
    });
}

#[test]
fn nested_files_are_compressed() {
    block_on(async {
        let app =
            TestServer::new(static_compiled!("./tests/fixtures/compressible", compress)).await;
        app.get("/subdir/nested.html")
            .with_request_header("accept-encoding", "br")
            .await
            .assert_ok()
            .assert_header("content-encoding", "br")
            .assert_header("vary", "Accept-Encoding");
    });
}

#[test]
fn explicit_subset_only_bakes_listed_encodings() {
    // gzip-only handler: brotli-accepting client falls back to identity.
    block_on(async {
        let app = TestServer::new(static_compiled!(
            "./tests/fixtures/compressible",
            compress = [Gzip]
        ))
        .await;
        app.get("/lorem.html")
            .with_request_header("accept-encoding", "br")
            .await
            .assert_ok()
            .assert_no_header("content-encoding")
            .assert_header("vary", "Accept-Encoding");
    });
}

#[test]
fn explicit_subset_serves_listed_encoding() {
    block_on(async {
        let app = TestServer::new(static_compiled!(
            "./tests/fixtures/compressible",
            compress = [Gzip]
        ))
        .await;
        app.get("/lorem.html")
            .with_request_header("accept-encoding", "gzip")
            .await
            .assert_ok()
            .assert_header("content-encoding", "gzip")
            .assert_header("vary", "Accept-Encoding");
    });
}

#[test]
fn no_compress_arg_emits_no_vary_or_content_encoding() {
    // Even with the compression feature on, omitting `compress` from the
    // macro means no variants are baked and Vary stays off.
    block_on(async {
        let app = TestServer::new(static_compiled!("./tests/fixtures/compressible")).await;
        app.get("/lorem.html")
            .with_request_header("accept-encoding", "br")
            .await
            .assert_ok()
            .assert_no_header("content-encoding")
            .assert_no_header("vary");
    });
}

#[test]
fn vary_appended_to_upstream_value() {
    // An upstream handler stamps Vary; the static-compiled handler must
    // append, not overwrite.
    let inject_vary = |conn: Conn| async { conn.with_response_header(Vary, "User-Agent") };
    block_on(async {
        let app = TestServer::new((
            inject_vary,
            static_compiled!("./tests/fixtures/compressible", compress),
        ))
        .await;
        app.get("/lorem.html")
            .with_request_header("accept-encoding", "br")
            .await
            .assert_ok()
            .assert_header("content-encoding", "br")
            .assert_header("vary", "User-Agent, Accept-Encoding");
    });
}

#[test]
fn encoding_token_round_trip() {
    assert_eq!(Encoding::Brotli.token(), "br");
    assert_eq!(Encoding::Zstd.token(), "zstd");
    assert_eq!(Encoding::Gzip.token(), "gzip");
}
