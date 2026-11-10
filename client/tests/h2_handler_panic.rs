//! A server handler that panics on an HTTP/2 stream answers with a `500` and leaves the connection
//! serving other requests. This rests on the runtime dropping the handler's conn while the panic
//! unwinds, so it runs on each runtime's real executor.

use trillium::Conn;
use trillium_client::{Client, Version};
use trillium_testing::{TestResult, harness};

async fn handler(conn: Conn) -> Conn {
    if conn.path() == "/panic" {
        panic!("handler panicked");
    }
    conn.ok("fine")
}

async fn assert_panic_answers_500(client: Client) -> TestResult {
    for _ in 0..2 {
        let conn = client
            .get("/panic")
            .with_http_version(Version::Http2)
            .await?;
        assert_eq!(conn.http_version(), Version::Http2);
        assert_eq!(conn.status().unwrap(), 500);

        let mut conn = client.get("/").with_http_version(Version::Http2).await?;
        assert_eq!(conn.status().unwrap(), 200);
        assert_eq!(conn.response_body().read_string().await?, "fine");
    }
    Ok(())
}

#[trillium_testing::test(harness)]
async fn smol_handler_panic_answers_500() -> TestResult {
    let server = trillium_smol::config()
        .with_host("localhost")
        .with_port(0)
        .spawn(handler);
    let port = server.info().await.tcp_socket_addr().unwrap().port();
    let client = Client::new(trillium_smol::ClientConfig::default())
        .with_base(format!("http://localhost:{port}"));
    assert_panic_answers_500(client).await?;
    server.shut_down().await;
    Ok(())
}

// tokio's networking needs its own runtime context, which the test harness doesn't provide.
#[test]
fn tokio_handler_panic_answers_500() -> TestResult {
    trillium_tokio::TokioRuntime::default().block_on(async {
        let server = trillium_tokio::config()
            .with_host("localhost")
            .with_port(0)
            .spawn(handler);
        let port = server.info().await.tcp_socket_addr().unwrap().port();
        let client = Client::new(trillium_tokio::ClientConfig::default())
            .with_base(format!("http://localhost:{port}"));
        assert_panic_answers_500(client).await?;
        server.shut_down().await;
        Ok(())
    })
}
