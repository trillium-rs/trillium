//! `H1Connection` reports how a connection ended, and its `H1Close` closes gracefully.
use futures_lite::{AsyncReadExt, future::poll_once};
use std::{net::Shutdown, sync::Arc};
use test_harness::test;
use trillium_http::{
    Conn, HttpContext,
    h1::{H1Close, H1Connection, H1ConnectionResult},
};
use trillium_testing::{TestTransport, harness};

async fn handler(mut conn: Conn<TestTransport>) -> Conn<TestTransport> {
    conn.set_status(200);
    conn.set_response_body("ok");
    conn
}

async fn serve(server: TestTransport) -> H1Close<TestTransport> {
    match H1Connection::new(Arc::new(HttpContext::new()), server)
        .run(handler)
        .await
    {
        H1ConnectionResult::Close(close) => close,
        other => panic!("expected the connection to close, got {other:?}"),
    }
}

#[test(harness)]
async fn close_shuts_down_sending_then_waits_for_the_client() {
    let (mut client, server) = TestTransport::new();
    client.write_all("GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n");
    let mut close = serve(server).await;

    assert!(
        poll_once(&mut close).await.is_none(),
        "close waits for the client to close its side"
    );
    let mut response = String::new();
    client.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");

    client.write_all("bytes sent after the response");
    assert!(poll_once(&mut close).await.is_none());
    assert!(
        client.write_side().current(),
        "close reads and discards what the client sends while it waits"
    );

    client.shutdown(Shutdown::Write);
    assert!(
        poll_once(&mut close).await.is_some(),
        "close finishes once the client closes its side"
    );
}

#[test(harness)]
async fn close_after_the_client_closes_completes_immediately() {
    let (client, server) = TestTransport::new();
    client.write_all("GET / HTTP/1.1\r\nHost: example.com\r\n\r\n");
    client.shutdown(Shutdown::Write);
    let mut close = serve(server).await;

    assert!(poll_once(&mut close).await.is_some());
}

/// A client that keeps data available must not keep the close from returning `Pending`, or a
/// timeout racing it would never get polled.
#[test(harness)]
async fn close_yields_while_the_client_keeps_sending() {
    let (client, server) = TestTransport::new();
    client.write_all("GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n");
    let mut close = serve(server).await;

    client.write_all(vec![b'x'; 1 << 20]);
    assert!(poll_once(&mut close).await.is_none());
    assert!(
        !client.write_side().current(),
        "close yielded before draining everything the client sent"
    );
}

#[test(harness)]
async fn disconnecting_mid_request_is_an_error() {
    let (client, server) = TestTransport::new();
    client.write_all("GET / HTTP/1.1\r\nHost: exa");
    client.shutdown(Shutdown::Write);

    let result = H1Connection::new(Arc::new(HttpContext::new()), server)
        .run(handler)
        .await;
    assert!(matches!(result, H1ConnectionResult::Error(_)), "{result:?}");
}
