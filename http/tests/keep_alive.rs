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
        "GET /1 HTTP/1.1\r\nHost: _\r\nConnection: keep-alive\r\nConnection: close\r\n\r\n\
         GET /2 HTTP/1.1\r\nHost: _\r\n\r\n",
    )
    .await;
    assert_eq!(response_count(&responses), 1, "{responses:?}");
}

/// Sanity check the harness: without a `close` token the server stays persistent and answers
/// both pipelined requests.
#[test(harness)]
async fn keep_alive_serves_pipelined_requests() {
    let responses = drive(
        "GET /1 HTTP/1.1\r\nHost: _\r\n\r\n\
         GET /2 HTTP/1.1\r\nHost: _\r\n\r\n",
    )
    .await;
    assert_eq!(response_count(&responses), 2, "{responses:?}");
}
