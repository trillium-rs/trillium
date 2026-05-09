//! Integration tests for HTTP Range request handling in `trillium-static`.
//!
//! Run only when a runtime feature is enabled (matching `handler_tests.rs`).

#![cfg(any(feature = "smol", feature = "tokio", feature = "async-std"))]

use std::{fs, path::PathBuf};
use tempfile::TempDir;
use trillium_static::StaticFileHandler;
use trillium_testing::{TestServer, block_on};

/// Set up a fixture with a single moderately-sized file. Returns
/// `(outer, www, content)`.
fn setup() -> (TempDir, PathBuf, String) {
    let content: String = (0..2000)
        .map(|i| char::from((b'a' + (i % 26) as u8) as char))
        .collect();
    let outer = TempDir::new().unwrap();
    let www = outer.path().join("www");
    fs::create_dir(&www).unwrap();
    fs::write(www.join("data.txt"), &content).unwrap();
    (outer, www, content)
}

#[test]
fn accept_ranges_advertised() {
    let (_outer, www, _) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        app.get("/data.txt")
            .await
            .assert_ok()
            .assert_header("accept-ranges", "bytes");
    });
}

#[test]
fn simple_range_returns_206() {
    let (_outer, www, content) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        app.get("/data.txt")
            .with_request_header("range", "bytes=0-99")
            .await
            .assert_status(206)
            .assert_header("content-range", "bytes 0-99/2000")
            .assert_header("accept-ranges", "bytes")
            .assert_body(&content[..=99]);
    });
}

#[test]
fn suffix_range() {
    let (_outer, www, content) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        app.get("/data.txt")
            .with_request_header("range", "bytes=-50")
            .await
            .assert_status(206)
            .assert_header("content-range", "bytes 1950-1999/2000")
            .assert_body(&content[1950..]);
    });
}

#[test]
fn open_ended_range() {
    let (_outer, www, content) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        app.get("/data.txt")
            .with_request_header("range", "bytes=1500-")
            .await
            .assert_status(206)
            .assert_header("content-range", "bytes 1500-1999/2000")
            .assert_body(&content[1500..]);
    });
}

#[test]
fn end_past_total_clamps() {
    let (_outer, www, content) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        app.get("/data.txt")
            .with_request_header("range", "bytes=0-99999")
            .await
            .assert_status(206)
            .assert_header("content-range", "bytes 0-1999/2000")
            .assert_body(&content);
    });
}

#[test]
fn out_of_bounds_returns_416() {
    let (_outer, www, _) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        app.get("/data.txt")
            .with_request_header("range", "bytes=99999-")
            .await
            .assert_status(416)
            .assert_header("content-range", "bytes */2000")
            .assert_body("");
    });
}

#[test]
fn multi_range_falls_through_to_full_body() {
    let (_outer, www, content) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        app.get("/data.txt")
            .with_request_header("range", "bytes=0-100,200-300")
            .await
            .assert_ok()
            .assert_no_header("content-range")
            .assert_body(&content);
    });
}

#[test]
fn invalid_range_unit_falls_through() {
    let (_outer, www, content) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        app.get("/data.txt")
            .with_request_header("range", "seconds=0-10")
            .await
            .assert_ok()
            .assert_no_header("content-range")
            .assert_body(&content);
    });
}

#[test]
fn if_range_matching_last_modified_honors_range() {
    // The static crate's etag from file metadata is weak, so it can't be
    // used for If-Range (strong-comparison only per RFC 9110 14.1.2). The
    // Last-Modified date is the practical validator for ranged requests
    // against this handler.
    let (_outer, www, content) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        let conn = app.get("/data.txt").await;
        let last_modified = conn
            .response_headers()
            .get_str(trillium::KnownHeaderName::LastModified)
            .expect("Last-Modified set on full response")
            .to_owned();

        app.get("/data.txt")
            .with_request_header("range", "bytes=0-49")
            .with_request_header("if-range", last_modified)
            .await
            .assert_status(206)
            .assert_body(&content[..=49]);
    });
}

#[test]
fn if_range_weak_etag_never_matches() {
    // The static crate's etag is weak (file-metadata-based). A weak etag
    // in If-Range must never match per spec, so the client gets a 200 full
    // body instead of a 206.
    let (_outer, www, content) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        let conn = app.get("/data.txt").await;
        let etag = conn
            .response_headers()
            .get_str(trillium::KnownHeaderName::Etag)
            .expect("etag set on full response")
            .to_owned();
        assert!(etag.starts_with("W/"), "expected a weak etag, got {etag}");

        app.get("/data.txt")
            .with_request_header("range", "bytes=0-49")
            .with_request_header("if-range", etag)
            .await
            .assert_ok()
            .assert_no_header("content-range")
            .assert_body(&content);
    });
}

#[test]
fn if_range_nonmatching_falls_through_to_full_body() {
    let (_outer, www, content) = setup();
    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www)).await;
        app.get("/data.txt")
            .with_request_header("range", "bytes=0-49")
            .with_request_header("if-range", "\"some-other-etag\"")
            .await
            .assert_ok()
            .assert_no_header("content-range")
            .assert_body(&content);
    });
}

#[test]
fn range_bypasses_sidecar_selection() {
    // With precompressed sidecars enabled, a Range request on the source
    // file should serve identity bytes (not the .br sidecar), with no
    // Content-Encoding header.
    let outer = TempDir::new().unwrap();
    let www = outer.path().join("www");
    fs::create_dir(&www).unwrap();
    fs::write(www.join("page.html"), "AAAAAAAAAAAAAAAAAAAA").unwrap();
    fs::write(www.join("page.html.br"), "BR-SIDECAR-PAYLOAD").unwrap();

    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www).with_precompressed()).await;
        app.get("/page.html")
            .with_request_header("range", "bytes=0-9")
            .with_request_header("accept-encoding", "br")
            .await
            .assert_status(206)
            .assert_header("content-range", "bytes 0-9/20")
            .assert_no_header("content-encoding")
            .assert_body("AAAAAAAAAA");
    });
}

#[test]
fn range_on_index_file() {
    // Range on a directory request resolves through the configured index
    // file and slices that file.
    let outer = TempDir::new().unwrap();
    let www = outer.path().join("www");
    fs::create_dir(&www).unwrap();
    fs::write(www.join("index.html"), "0123456789ABCDEFGHIJ").unwrap();

    block_on(async {
        let app = TestServer::new(StaticFileHandler::new(&www).with_index_file("index.html")).await;
        app.get("/")
            .with_request_header("range", "bytes=0-4")
            .await
            .assert_status(206)
            .assert_header("content-range", "bytes 0-4/20")
            .assert_body("01234");
    });
}
