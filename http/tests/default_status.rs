//! The status an unhandled conn sends when a handler leaves it unset: `404` for most methods,
//! but `501 Not Implemented` for CONNECT, which an origin server does not tunnel.
use std::{net::Shutdown, sync::Arc};
use test_harness::test;
use trillium_http::{Conn, HttpContext, Swansong};
use trillium_testing::{RuntimeTrait, TestTransport, harness};

async fn unhandled_status_line(request: &str) -> String {
    let runtime = trillium_testing::runtime();
    let (client, server) = TestTransport::new();
    let swansong = Swansong::new();
    let context = Arc::new(HttpContext::new());
    let res = runtime.spawn({
        let context = context.clone();
        async move {
            context
                .run(server, |conn: Conn<TestTransport>| async move { conn })
                .await
        }
    });

    client.write_all(request);
    client.shutdown(Shutdown::Write);
    res.await.unwrap().unwrap();
    let response = client.read_available_string().await;
    swansong.shut_down();

    response.lines().next().unwrap_or_default().to_string()
}

#[test(harness)]
async fn unhandled_get_is_404() {
    assert_eq!(
        unhandled_status_line("GET / HTTP/1.1\r\nHost: example.com\r\n\r\n").await,
        "HTTP/1.1 404 Not Found"
    );
}

#[test(harness)]
async fn unhandled_connect_is_501() {
    assert_eq!(
        unhandled_status_line("CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n")
            .await,
        "HTTP/1.1 501 Not Implemented"
    );
}
