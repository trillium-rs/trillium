//! A request head longer than `head_max_len` is answered with `431` before the connection closes.
use std::{net::Shutdown, sync::Arc};
use test_harness::test;
use trillium_http::{Conn, HttpConfig, HttpContext};
use trillium_testing::{RuntimeTrait, TestTransport, harness};

const OVERSIZED: &str = "GET / HTTP/1.1\r\nHost: example.com\r\nX-Padding: \
                         aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n\r\n";

async fn handler(mut conn: Conn<TestTransport>) -> Conn<TestTransport> {
    conn.set_status(200);
    conn.set_response_body("ok");
    conn
}

async fn drive(requests: &str) -> String {
    let runtime = trillium_testing::runtime();
    let (client, server) = TestTransport::new();
    let context =
        Arc::new(HttpContext::new().with_config(HttpConfig::default().with_head_max_len(64)));
    let res = runtime.spawn(async move { context.run(server, handler).await });

    client.write_all(requests);
    client.shutdown(Shutdown::Write);
    res.await.unwrap().unwrap();
    client.read_available_string().await
}

#[test(harness)]
async fn oversized_first_request_head_gets_431() {
    let response = drive(OVERSIZED).await;
    assert!(
        response.starts_with("HTTP/1.1 431 Request Header Fields Too Large\r\n"),
        "{response}"
    );
    assert!(response.contains("Connection: close\r\n"), "{response}");
}

#[test(harness)]
async fn oversized_keep_alive_request_head_gets_431() {
    let response = drive(&format!(
        "GET / HTTP/1.1\r\nHost: example.com\r\n\r\n{OVERSIZED}"
    ))
    .await;
    let ok = response.find("HTTP/1.1 200 OK\r\n").expect(&response);
    let too_large = response
        .find("HTTP/1.1 431 Request Header Fields Too Large\r\n")
        .expect(&response);
    assert!(ok < too_large, "{response}");
    assert!(
        response[too_large..].contains("Connection: close\r\n"),
        "{response}"
    );
}
