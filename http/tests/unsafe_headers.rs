use indoc::{formatdoc, indoc};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use test_harness::test;
use trillium_http::{Conn, KnownHeaderName, SERVER, ServerConfig};
use trillium_testing::{RuntimeTrait, TestResult, TestTransport, harness};

const TEST_DATE: &str = "Tue, 21 Nov 2023 21:27:21 GMT";

async fn handler(mut conn: Conn<TestTransport>) -> Conn<TestTransport> {
    conn.set_status(200);
    conn.set_response_body("response: 0123456789");
    conn.response_headers_mut()
        .insert(KnownHeaderName::Date, TEST_DATE);
    conn.response_headers_mut().insert(
        KnownHeaderName::Connection,
        "close\r\nGET / HTTP/1.1\r\nHost: example.com\r\n\r\n",
    );
    conn.response_headers_mut().insert("Bad\r\nHeader", "true");
    conn
}

#[test(harness)]
async fn bad_headers() -> TestResult {
    let (client, server) = TestTransport::new();
    let runtime = trillium_testing::runtime();
    let server_config = Arc::new(ServerConfig::default());
    let handle = runtime.spawn(server_config.run(server, handler));

    client.write_all(indoc! {"
        GET / HTTP/1.1\r
        Host: example.com\r
        Connection: close\r
        \r
    "});

    let expected_response = formatdoc! {"
        HTTP/1.1 200 OK\r
        Date: {TEST_DATE}\r
        Content-Length: 20\r
        Server: {SERVER}\r
        \r
        response: 0123456789\
    "};

    assert_eq!(client.read_available_string().await, expected_response);

    handle.await.unwrap().unwrap();

    Ok(())
}
