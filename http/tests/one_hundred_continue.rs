use indoc::{formatdoc, indoc};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use test_harness::test;
use trillium_http::{Conn, KnownHeaderName, SERVER, ServerConfig};
use trillium_testing::{RuntimeTrait, TestResult, TestTransport, harness};

const TEST_DATE: &str = "Tue, 21 Nov 2023 21:27:21 GMT";

async fn handler(mut conn: Conn<TestTransport>) -> Conn<TestTransport> {
    conn.set_status(200);
    let request_body = conn.request_body().await.read_string().await.unwrap();
    conn.set_response_body(format!("response: {request_body}"));
    conn.response_headers_mut()
        .insert(KnownHeaderName::Connection, "close");
    conn.response_headers_mut()
        .insert(KnownHeaderName::Date, TEST_DATE);
    conn
}

#[test(harness)]
async fn one_hundred_continue() -> TestResult {
    let (client, server) = TestTransport::new();
    let runtime = trillium_testing::runtime();
    let server_config = Arc::new(ServerConfig::default());
    let handle = runtime.spawn(server_config.run(server, handler));

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
        Server: {SERVER}\r
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
    let server_config = Arc::new(ServerConfig::default());
    let handle = runtime.spawn(server_config.run(server, handler));

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
        Server: {SERVER}\r
        \r
        response: 0123456789\
    "};

    assert_eq!(client.read_available_string().await, expected_response);
    handle.await.unwrap().unwrap();
    Ok(())
}
