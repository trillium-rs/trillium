//! End-to-end tests for the synthetic-response surface on `Conn`:
//! `set_status` / `with_status`, `halt`, `set_response_body` / `with_response_body`.
//!
//! The transport here always responds 500, so any test that succeeds with a 200 is proving
//! that the network round-trip was skipped.

use futures_lite::AsyncReadExt;
use trillium_client::{Client, Status};
use trillium_http::KnownHeaderName::{ContentLength, ContentType};
use trillium_testing::{ServerConnector, TestResult, harness, test};

fn test_client() -> Client {
    Client::new(ServerConnector::new(Status::InternalServerError))
}

#[test(harness)]
async fn halted_conn_skips_network_and_returns_synthetic_state() -> TestResult {
    let client = test_client();
    let mut conn = client
        .get("http://synthetic.invalid/")
        .with_status(Status::Ok)
        .with_response_body("hello from synthesis");
    conn.response_headers_mut()
        .insert(ContentType, "text/plain; charset=utf-8");
    conn.response_headers_mut().insert(ContentLength, "20");
    conn.halt();

    let mut conn = conn.await?;

    assert_eq!(conn.status(), Some(Status::Ok));
    assert_eq!(
        conn.response_headers().get_str(ContentType),
        Some("text/plain; charset=utf-8"),
    );
    assert_eq!(conn.response_body().content_length(), Some(20));
    assert_eq!(
        conn.response_body().read_string().await?,
        "hello from synthesis",
    );
    Ok(())
}

#[test(harness)]
async fn synthetic_body_honors_user_set_max_len() -> TestResult {
    let client = test_client();
    let mut conn = client
        .get("http://synthetic.invalid/")
        .with_status(Status::Ok)
        .with_response_body(vec![0u8; 1024]);
    conn.halt();
    let mut conn = conn.await?;

    let result = conn.response_body().with_max_len(64).read_bytes().await;
    assert!(result.is_err(), "expected too-long error, got {result:?}");
    Ok(())
}

#[test(harness)]
async fn synthetic_body_streams_via_async_read() -> TestResult {
    use futures_lite::io::Cursor;
    use trillium_http::Body;

    let client = test_client();
    let mut conn = client
        .get("http://synthetic.invalid/")
        .with_status(Status::Ok)
        .with_response_body(Body::new_streaming(Cursor::new(b"streamed".to_vec()), Some(8)));
    conn.halt();
    let mut conn = conn.await?;

    let mut out = String::new();
    conn.response_body().read_to_string(&mut out).await?;
    assert_eq!(out, "streamed");
    Ok(())
}

#[test(harness)]
async fn synthesis_without_halt_still_hits_the_network() -> TestResult {
    // Sanity check: set response body but DON'T halt — the server connector responds 500.
    // The synthetic body is preserved on the conn because the network exec doesn't touch
    // synthetic_response_body, but the actual response status comes from the network.
    let client = test_client();
    let conn = client
        .get("http://synthetic.invalid/")
        .with_response_body("ignored")
        .await?;
    assert_eq!(conn.status(), Some(Status::InternalServerError));
    Ok(())
}
