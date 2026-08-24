//! Server-side connection-persistence (`should_close`) behavior over a raw transport.
use std::{net::Shutdown, sync::Arc};
use test_harness::test;
use trillium_http::{Conn, HttpContext};
use trillium_testing::{RuntimeTrait, TestTransport, harness};

async fn handler(mut conn: Conn<TestTransport>) -> Conn<TestTransport> {
    conn.set_status(200);
    conn.set_response_body("ok");
    conn
}

/// Count how many response heads the server wrote.
fn response_count(responses: &str) -> usize {
    responses.matches("HTTP/1.1 200").count()
}

async fn drive(requests: &str) -> String {
    let runtime = trillium_testing::runtime();
    let (client, server) = TestTransport::new();
    let context = Arc::new(HttpContext::new());
    let res = runtime.spawn(async move { context.run(server, handler).await });

    client.write_all(requests);
    client.shutdown(Shutdown::Write);
    res.await.unwrap().unwrap();
    client.read_available_string().await
}

/// A `Connection: close` token split across two header lines must still close the connection.
/// `get_str` returns `None` for a header present on more than one line, so the older
/// single-line lookup missed the `close` token and kept a connection the peer asked to close.
#[test(harness)]
async fn connection_close_split_across_lines() {
    let responses = drive(
        "GET /1 HTTP/1.1\r\nHost: _\r\nConnection: keep-alive\r\nConnection: close\r\n\r\nGET /2 \
         HTTP/1.1\r\nHost: _\r\n\r\n",
    )
    .await;
    assert_eq!(response_count(&responses), 1, "{responses:?}");
}

/// Sanity check the harness: without a `close` token the server stays persistent and answers
/// both pipelined requests.
#[test(harness)]
async fn keep_alive_serves_pipelined_requests() {
    let responses =
        drive("GET /1 HTTP/1.1\r\nHost: _\r\n\r\nGET /2 HTTP/1.1\r\nHost: _\r\n\r\n").await;
    assert_eq!(response_count(&responses), 2, "{responses:?}");
}

/// Regression: a chunk-size token within 2 of `u64::MAX` used to wrap `chunk_size + 2` in
/// release builds, truncating the chunk so an attacker could reframe trailing bytes as the
/// next pipelined request (request smuggling). The oversized size line must be rejected and
/// only the first request served.
#[test(harness)]
async fn chunk_size_overflow_does_not_smuggle_second_request() {
    let runtime = trillium_testing::runtime();
    let (client, server) = TestTransport::new();
    let res = runtime.spawn(async move {
        Arc::new(HttpContext::new())
            .run(server, |mut conn: Conn<TestTransport>| async move {
                conn.set_status(200);
                conn.set_response_body(format!("handled {}", conn.path()));
                conn
            })
            .await
    });

    client.write_all(
        b"POST /1 HTTP/1.1\r\nHost: _\r\nTransfer-Encoding: chunked\r\n\r\n\
          FFFFFFFFFFFFFFFF\r\nX0\r\n\r\nGET /2 HTTP/1.1\r\nHost: _\r\n\r\n",
    );
    client.shutdown(std::net::Shutdown::Write);

    // The malformed chunked body surfaces as a server-side drain/read error after the
    // first response; that outcome is expected, unlike a second response.
    let _ = res.await.unwrap();
    let responses = client.read_available_string().await;
    assert!(responses.contains("handled /1"), "{responses:?}");
    assert!(!responses.contains("handled /2"), "{responses:?}");
    assert_eq!(response_count(&responses), 1, "{responses:?}");
}
