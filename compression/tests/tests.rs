use trillium::{Conn, KnownHeaderName::{AcceptEncoding, ContentEncoding, ContentType}};
use trillium_compression::{Compression, Level};
use trillium_testing::{TestServer, harness, test};

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

    let response = app
        .get("/")
        .with_request_header(AcceptEncoding, "br")
        .await;
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

    let response = app
        .get("/")
        .with_request_header(AcceptEncoding, "br")
        .await;
    response.assert_no_header("content-encoding");
    response.assert_header("content-length", "41");
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
