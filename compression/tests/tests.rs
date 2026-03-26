use trillium::KnownHeaderName::AcceptEncoding;
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
async fn test() {
    let app = TestServer::new((trillium_compression::compression(), COMPRESSIBLE_CONTENT)).await;

    assert_eq!(COMPRESSIBLE_CONTENT.len(), 500);

    app.get("/")
        .with_request_header(AcceptEncoding, "zstd")
        .await
        .assert_header("content-length", "68")
        .assert_header("vary", "Accept-Encoding")
        .assert_header("content-encoding", "zstd");

    app.get("/")
        .with_request_header(AcceptEncoding, "br")
        .await
        .assert_header("content-length", "51")
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
        .assert_header("content-length", "51")
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
