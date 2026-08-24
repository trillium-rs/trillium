use indoc::{formatdoc, indoc};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use test_harness::test;
use trillium_http::{Conn, HttpContext, KnownHeaderName, SERVER_HEADER};
use trillium_testing::{RuntimeTrait, TestResult, TestTransport, harness};

const TEST_DATE: &str = "Tue, 21 Nov 2023 21:27:21 GMT";

async fn handler(mut conn: Conn<TestTransport>) -> Conn<TestTransport> {
    conn.set_status(200);
    let request_body = conn.request_body().read_string().await.unwrap();
    conn.set_response_body(format!("response: {request_body}"));
    conn.response_headers_mut()
        .insert(KnownHeaderName::Connection, "close")
        .insert(KnownHeaderName::Date, TEST_DATE)
        .insert(KnownHeaderName::Server, SERVER_HEADER);
    conn
}

#[test(harness)]
async fn one_hundred_continue() -> TestResult {
    let (client, server) = TestTransport::new();
    let runtime = trillium_testing::runtime();
    let context = Arc::new(HttpContext::default());
    let handle = runtime.spawn(context.run(server, handler));

    client.write_all(indoc! {"
        POST / HTTP/1.1\r
        Expect: 100-continue\r
        Host: example.com\r
        Content-Length: 10\r
        \r
    "});

    assert_eq!(
        client.read_available_string().await,
        "HTTP/1.1 100 Continue\r\n\r\n"
    );

    client.write_all(b"0123456789");

    let expected_response = formatdoc! {"
        HTTP/1.1 200 OK\r
        Date: {TEST_DATE}\r
        Connection: close\r
        Content-Length: 20\r
        Server: {SERVER_HEADER}\r
        \r
        response: 0123456789\
    "};

    assert_eq!(client.read_available_string().await, expected_response);
    handle.await.unwrap().unwrap();
    Ok(())
}

#[test(harness)]
async fn one_hundred_continue_http_one_dot_zero() -> TestResult {
    let (client, server) = TestTransport::new();
    let runtime = trillium_testing::runtime();
    let context = Arc::new(HttpContext::default());
    let handle = runtime.spawn(context.run(server, handler));

    client.write_all(indoc! { "
        POST / HTTP/1.0\r
        Expect: 100-continue\r
        Host: example.com\r
        Content-Length: 10\r
        \r
    "});

    client.write_all(b"0123456789");

    let expected_response = formatdoc! {"
        HTTP/1.0 200 OK\r
        Date: {TEST_DATE}\r
        Connection: close\r
        Content-Length: 20\r
        Server: {SERVER_HEADER}\r
        \r
        response: 0123456789\
    "};

    assert_eq!(client.read_available_string().await, expected_response);
    handle.await.unwrap().unwrap();
    Ok(())
}

/// A handler that answers an `Expect: 100-continue` request without reading its body skips
/// draining before the next request head (draining would block against a compliant client
/// still waiting for `100 Continue`), so the connection must be closed rather than reused —
/// otherwise a client that sends the body anyway gets those bytes parsed as the next request.
#[test(harness)]
async fn unread_expect_body_closes_connection() -> TestResult {
    let (client, server) = TestTransport::new();
    let runtime = trillium_testing::runtime();
    let context = Arc::new(HttpContext::default());
    let handle = runtime.spawn(
        context.run(server, |mut conn: Conn<TestTransport>| async move {
            conn.set_status(204);
            conn.response_headers_mut()
                .insert(KnownHeaderName::Date, TEST_DATE)
                .insert(KnownHeaderName::Server, SERVER_HEADER);
            conn
        }),
    );

    client.write_all(indoc! {"
        POST / HTTP/1.1\r
        Expect: 100-continue\r
        Host: example.com\r
        Content-Length: 10\r
        \r
    "});
    // a non-compliant client sends the body bytes anyway, followed by what it intends as
    // a second pipelined request
    client.write_all(b"0123456789GET /next HTTP/1.1\r\nHost: _\r\n\r\n");
    client.shutdown(std::net::Shutdown::Write);

    handle.await.unwrap().unwrap();
    let response = client.read_available_string().await;

    assert!(
        response.starts_with("HTTP/1.1 204"),
        "expected the 204 response first: {response:?}"
    );
    assert!(
        !response.contains("100 Continue"),
        "no 100 Continue should have been sent: {response:?}"
    );
    assert!(
        !response.contains("/next"),
        "the trailing bytes were served as a second request: {response:?}"
    );
    assert_eq!(response.matches("HTTP/1.1").count(), 1, "{response:?}");
    Ok(())
}
