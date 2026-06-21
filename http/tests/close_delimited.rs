//! Close-delimited response framing (RFC 9112 §6.3): a `Connection: close` response with
//! an unknown-length streaming body carries neither `Content-Length` nor
//! `Transfer-Encoding`, and its body bytes go out raw rather than chunk-framed.
use futures_lite::io::Cursor;
use std::{net::Shutdown, sync::Arc};
use test_harness::test;
use trillium_http::{Body, Conn, HttpContext, KnownHeaderName};
use trillium_testing::{RuntimeTrait, TestTransport, harness};

fn streaming(content: &'static str) -> Body {
    Body::new_streaming(Cursor::new(content.as_bytes().to_vec()), None)
}

async fn drive(
    handler: impl Fn(Conn<TestTransport>) -> Conn<TestTransport> + Send + Sync + 'static,
) -> String {
    let runtime = trillium_testing::runtime();
    let (client, server) = TestTransport::new();
    let context = Arc::new(HttpContext::new());
    let res = runtime.spawn(async move {
        context
            .run(server, |conn| {
                let conn = handler(conn);
                async move { conn }
            })
            .await
    });

    client.write_all("GET / HTTP/1.1\r\nHost: _\r\n\r\n");
    client.shutdown(Shutdown::Write);
    res.await.unwrap().unwrap();
    client.read_available_string().await
}

#[test(harness)]
async fn connection_close_streaming_body_is_close_delimited() {
    let response = drive(|mut conn| {
        conn.set_status(200);
        conn.response_headers_mut()
            .insert(KnownHeaderName::Connection, "close");
        conn.set_response_body(streaming("hello"));
        conn
    })
    .await;

    let lower = response.to_ascii_lowercase();
    assert!(
        !lower.contains("transfer-encoding"),
        "close-delimited response must not chunk:\n{response:?}"
    );
    assert!(
        !lower.contains("content-length"),
        "close-delimited response must not have a content-length:\n{response:?}"
    );
    // Body bytes appear verbatim, with no hex chunk prefix or trailer terminator.
    assert!(
        response.ends_with("\r\n\r\nhello"),
        "body should be raw, not chunk-framed:\n{response:?}"
    );
}

#[test(harness)]
async fn streaming_body_without_close_still_chunks() {
    let response = drive(|mut conn| {
        conn.set_status(200);
        conn.set_response_body(streaming("hello"));
        conn
    })
    .await;

    let lower = response.to_ascii_lowercase();
    assert!(
        lower.contains("transfer-encoding: chunked"),
        "unknown-length keep-alive response should chunk:\n{response:?}"
    );
    assert!(
        response.ends_with("\r\n\r\n5\r\nhello\r\n0\r\n\r\n"),
        "body should be chunk-framed:\n{response:?}"
    );
}
