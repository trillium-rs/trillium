use futures_lite::io::Cursor;
use trillium::{
    Body, Conn,
    KnownHeaderName::{AcceptEncoding, ContentEncoding, ContentLength, ContentType},
};
use trillium_client::Client;
use trillium_compression::{Compression, Level};
use trillium_testing::{ServerConnector, TestResult, TestServer, harness, test};

static COMPRESSIBLE_CONTENT: &str = r#"
should be very compressible because it's repeated
should be very compressible because it's repeated
should be very compressible because it's repeated
should be very compressible because it's repeated
should be very compressible because it's repeated
should be very compressible because it's repeated
should be very compressible because it's repeated
should be very compressible because it's repeated
should be very compressible because it's repeated
should be very compressible because it's repeated"#;

/// Server app that emits [`COMPRESSIBLE_CONTENT`] as an unknown-length streaming body, driving the
/// `is_streaming()` branch of `encode()` rather than the static branch the other tests exercise.
fn streaming_app() -> impl trillium::Handler {
    (
        trillium_compression::compression(),
        |conn: Conn| async move {
            conn.with_body(Body::new_streaming(
                Cursor::new(COMPRESSIBLE_CONTENT.as_bytes()),
                None,
            ))
        },
    )
}

#[test(harness)]
async fn negotiation_and_default_levels() {
    let app = TestServer::new((trillium_compression::compression(), COMPRESSIBLE_CONTENT)).await;

    assert_eq!(COMPRESSIBLE_CONTENT.len(), 500);

    app.get("/")
        .with_request_header(AcceptEncoding, "zstd")
        .await
        .assert_header("content-length", "68")
        .assert_header("vary", "Accept-Encoding")
        .assert_header("content-encoding", "zstd");

    // Default brotli is now Level::Precise(4); was 51 at q11.
    app.get("/")
        .with_request_header(AcceptEncoding, "br")
        .await
        .assert_header("content-length", "48")
        .assert_header("vary", "Accept-Encoding")
        .assert_header("content-encoding", "br");

    app.get("/")
        .with_request_header(AcceptEncoding, "gzip")
        .await
        .assert_header("content-length", "77")
        .assert_header("vary", "Accept-Encoding")
        .assert_header("content-encoding", "gzip");

    app.get("/")
        .with_request_header(AcceptEncoding, "deflate")
        .await
        .assert_header("content-length", "500")
        .assert_no_header("vary")
        .assert_no_header("content-encoding");

    app.get("/")
        .with_request_header(AcceptEncoding, "br;q=0.5, gzip;q=0.75")
        .await
        .assert_header("content-length", "77")
        .assert_header("content-encoding", "gzip");

    app.get("/")
        .with_request_header(AcceptEncoding, "gzip;q=0.75, br;q=0.5, deflate")
        .await
        .assert_header("content-length", "77")
        .assert_header("content-encoding", "gzip");

    app.get("/")
        .with_request_header(AcceptEncoding, "deflate, gzip;q=0.75, br;q=0.95")
        .await
        .assert_header("content-length", "48")
        .assert_header("content-encoding", "br");

    app.get("/")
        .with_request_header(
            AcceptEncoding,
            "deflate, gzip;q=0.75, zstd;q=0.95, br;q=0.85",
        )
        .await
        .assert_header("content-length", "68")
        .assert_header("content-encoding", "zstd");
}

#[test(harness)]
async fn opt_in_to_max_brotli() {
    let app = TestServer::new((
        Compression::new().with_brotli_level(Level::Best),
        COMPRESSIBLE_CONTENT,
    ))
    .await;

    // Level::Best is brotli quality 11, the previous default — confirms
    // the level setter is wired through.
    app.get("/")
        .with_request_header(AcceptEncoding, "br")
        .await
        .assert_header("content-length", "51")
        .assert_header("content-encoding", "br");
}

#[test(harness)]
async fn skip_when_content_encoding_already_set() {
    let app = TestServer::new((
        trillium_compression::compression(),
        |conn: Conn| async move {
            conn.with_response_header(ContentEncoding, "br")
                .with_body(b"already-encoded".to_vec())
        },
    ))
    .await;

    let response = app.get("/").with_request_header(AcceptEncoding, "br").await;
    response.assert_header("content-encoding", "br");
    response.assert_header("content-length", "15");
}

#[test(harness)]
async fn skip_already_compressed_content_types() {
    let app = TestServer::new((
        trillium_compression::compression(),
        |conn: Conn| async move {
            conn.with_response_header(ContentType, "image/png")
                .with_body(b"\x89PNG-not-actually-but-shouldnt-be-touched".to_vec())
        },
    ))
    .await;

    let response = app.get("/").with_request_header(AcceptEncoding, "br").await;
    response.assert_no_header("content-encoding");
    response.assert_header("content-length", "41");
}

#[test(harness)]
async fn removes_stale_content_length_after_compression() {
    // An upstream (proxy, static sidecar, ...) may have set an explicit
    // content-length for the uncompressed body. After we compress, that
    // length is stale and must be dropped so the framework recomputes it.
    let app = TestServer::new((
        trillium_compression::compression(),
        |conn: Conn| async move {
            assert_eq!(COMPRESSIBLE_CONTENT.len(), 500);
            conn.with_response_header(ContentLength, "500")
                .with_body(COMPRESSIBLE_CONTENT)
        },
    ))
    .await;

    app.get("/")
        .with_request_header(AcceptEncoding, "br")
        .await
        .assert_header("content-encoding", "br")
        .assert_header("content-length", "48");
}

#[test(harness)]
async fn removes_stale_content_length_after_compression_streaming() {
    // Same bug as the static case, but through the streaming branch of
    // `encode()`: a compressed streaming body has unknown length, so the
    // stale content-length must be dropped and the response framed chunked.
    let app = TestServer::new((
        trillium_compression::compression(),
        |conn: Conn| async move {
            let body = Body::new_streaming(Cursor::new(COMPRESSIBLE_CONTENT.as_bytes()), None);
            conn.with_response_header(ContentLength, "500")
                .with_body(body)
        },
    ))
    .await;

    app.get("/")
        .with_request_header(AcceptEncoding, "br")
        .await
        .assert_header("content-encoding", "br")
        .assert_no_header("content-length");
}

#[test(harness)]
async fn svg_is_still_compressed() {
    let app = TestServer::new((
        trillium_compression::compression(),
        |conn: Conn| async move {
            conn.with_response_header(ContentType, "image/svg+xml")
                .with_body(COMPRESSIBLE_CONTENT)
        },
    ))
    .await;

    app.get("/")
        .with_request_header(AcceptEncoding, "br")
        .await
        .assert_header("content-encoding", "br");
}

#[test(harness)]
async fn streaming_bodies_compress_and_round_trip() -> TestResult {
    for algo in ["zstd", "br", "gzip"] {
        // A bare client observes the raw wire: the streaming body was actually compressed and,
        // having unknown length, framed chunked (no content-length).
        let client = Client::new(ServerConnector::new(streaming_app()));
        let mut conn = client
            .get("http://example.com/")
            .with_request_header(AcceptEncoding, algo)
            .await?;
        assert_eq!(
            conn.response_headers().get_str(ContentEncoding),
            Some(algo),
            "{algo}"
        );
        assert_eq!(
            conn.response_headers().get_str(ContentLength),
            None,
            "{algo}"
        );
        let wire = conn.response_body().read_bytes().await?;
        assert!(
            wire.len() < COMPRESSIBLE_CONTENT.len(),
            "{algo}: wire {} not smaller than plaintext {}",
            wire.len(),
            COMPRESSIBLE_CONTENT.len()
        );

        // A decoding client proves the compressed stream round-trips to the original plaintext —
        // ruling out an identity passthrough that the bare-client check alone couldn't detect.
        let client = Client::new(ServerConnector::new(streaming_app()))
            .with_handler(trillium_compression::client::Compression::new());
        let mut conn = client
            .get("http://example.com/")
            .with_request_header(AcceptEncoding, algo)
            .await?;
        assert_eq!(
            conn.response_body().read_string().await?,
            COMPRESSIBLE_CONTENT,
            "{algo}"
        );
    }

    Ok(())
}

#[test(harness)]
async fn streaming_svg_is_still_compressed() {
    let app = TestServer::new((
        trillium_compression::compression(),
        |conn: Conn| async move {
            conn.with_response_header(ContentType, "image/svg+xml")
                .with_body(Body::new_streaming(
                    Cursor::new(COMPRESSIBLE_CONTENT.as_bytes()),
                    None,
                ))
        },
    ))
    .await;

    app.get("/")
        .with_request_header(AcceptEncoding, "br")
        .await
        .assert_header("content-encoding", "br")
        .assert_no_header("content-length");
}
