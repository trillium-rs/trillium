//! Tests for the client-side `Compression` handler, exercising both directions through
//! [`ServerConnector`].
//!
//! - Response decoding uses the server-side [`trillium_compression::Compression`] handler to
//!   compress responses; the client-side handler decodes them.
//! - Request encoding uses an echo server that reflects the request's `Content-Encoding` back as
//!   the response `Content-Encoding` and echoes the raw request body — so the client's own inbound
//!   decode reconstructs the plaintext, proving the outbound encode round-trips.

use trillium::{Conn as ServerConn, KnownHeaderName::ContentType};
use trillium_client::{Client, KnownHeaderName::ContentEncoding, Status};
use trillium_compression::{CompressionAlgorithm, client::Compression};
use trillium_testing::{ServerConnector, TestResult, harness, test};

/// Long, highly compressible payload so the server-side handler actually shrinks it.
fn payload() -> String {
    "the quick brown fox jumps over the lazy dog. ".repeat(200)
}

/// Server app: compress responses, then emit the payload as `text/plain`.
fn app() -> impl trillium::Handler {
    (
        trillium_compression::Compression::new(),
        |conn: ServerConn| async move {
            conn.with_response_header(ContentType, "text/plain")
                .ok(payload())
        },
    )
}

/// Server app that reflects the request's `Content-Encoding` (into both a marker header and the
/// response `Content-Encoding`) and echoes the raw request body back. A correctly-encoded outbound
/// request therefore comes back through the client's own inbound decode as the original plaintext.
fn echo_app() -> impl trillium::Handler {
    |mut conn: ServerConn| async move {
        let request_encoding = conn
            .request_headers()
            .get_str(ContentEncoding)
            .map(String::from);
        let body = conn.request_body().read_bytes().await.unwrap();
        if let Some(encoding) = request_encoding {
            conn.response_headers_mut()
                .insert("x-request-content-encoding", encoding.clone())
                .insert(ContentEncoding, encoding);
        }
        conn.ok(body)
    }
}

#[test(harness)]
async fn decodes_compressed_response() -> TestResult {
    let client = Client::new(ServerConnector::new(app())).with_handler(Compression::new());

    let mut conn = client.get("http://example.com/").await?;
    assert_eq!(conn.status(), Some(Status::Ok));

    // The client handler stripped Content-Encoding after decoding.
    assert_eq!(conn.response_headers().get_str(ContentEncoding), None);

    // The caller reads plaintext, transparently decoded.
    assert_eq!(conn.response_body().read_string().await?, payload());

    Ok(())
}

#[test(harness)]
async fn server_actually_compresses_the_wire() -> TestResult {
    // Same server, but a bare client with no decode handler — we observe the raw wire body to
    // prove the decode test above isn't passing via an uncompressed identity round-trip.
    let client = Client::new(ServerConnector::new(app()));

    let mut conn = client
        .get("http://example.com/")
        .with_request_header("accept-encoding", "zstd")
        .await?;

    assert_eq!(
        conn.response_headers().get_str(ContentEncoding),
        Some("zstd")
    );

    let wire = conn.response_body().read_bytes().await?;
    assert!(
        wire.len() < payload().len(),
        "expected compressed wire body ({} bytes) to be smaller than plaintext ({} bytes)",
        wire.len(),
        payload().len(),
    );

    Ok(())
}

#[test(harness)]
async fn compresses_request_with_default_encoding() -> TestResult {
    let client = Client::new(ServerConnector::new(echo_app()))
        .with_handler(Compression::new().with_default_encoding(CompressionAlgorithm::Zstd));

    let mut conn = client
        .post("http://example.com/")
        .with_body(payload())
        .await?;

    // Server received a zstd-compressed request body...
    assert_eq!(
        conn.response_headers()
            .get_str("x-request-content-encoding"),
        Some("zstd")
    );
    // ...which round-trips back to the original plaintext through our own inbound decode.
    assert_eq!(conn.response_body().read_string().await?, payload());

    Ok(())
}

#[test(harness)]
async fn per_request_state_selects_encoding() -> TestResult {
    // No handler default: the per-conn state is the entire opt-in signal.
    let client = Client::new(ServerConnector::new(echo_app())).with_handler(Compression::new());

    let mut conn = client
        .post("http://example.com/")
        .with_body(payload())
        .with_state(CompressionAlgorithm::Gzip)
        .await?;

    assert_eq!(
        conn.response_headers()
            .get_str("x-request-content-encoding"),
        Some("gzip")
    );
    assert_eq!(conn.response_body().read_string().await?, payload());

    Ok(())
}

#[test(harness)]
async fn identity_state_opts_out_of_default() -> TestResult {
    let client = Client::new(ServerConnector::new(echo_app()))
        .with_handler(Compression::new().with_default_encoding(CompressionAlgorithm::Zstd));

    let mut conn = client
        .post("http://example.com/")
        .with_body(payload())
        .with_state(CompressionAlgorithm::Identity)
        .await?;

    // Identity overrides the default: the request body went out uncompressed.
    assert_eq!(
        conn.response_headers()
            .get_str("x-request-content-encoding"),
        None
    );
    assert_eq!(conn.response_body().read_string().await?, payload());

    Ok(())
}
