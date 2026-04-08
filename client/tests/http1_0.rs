use async_channel::Sender;
use futures_lite::io::Cursor;
use indoc::formatdoc;
use pretty_assertions::assert_eq;
use std::{
    future::{Future, IntoFuture},
    io,
    net::SocketAddr,
};
use test_harness::test;
use trillium_client::{Body, Client, Conn, Error, Status, USER_AGENT, Version};
use trillium_server_common::{Connector, Url};
use trillium_testing::{RuntimeTrait, TestResult, TestTransport, harness};

// Pattern for these tests: write the response before awaiting the conn. The body is always
// flushed to the transport before parse_head runs, so after conn_fut.await the transport
// buffer contains the complete request (head + body). Reading after the await is stable
// regardless of when the task flushes relative to our reads.

#[test(harness)]
async fn http1_0_get() -> TestResult {
    let (transport, conn_fut) = test_conn(|client| {
        client
            .get("http://example.com/path")
            .with_http_version(Version::Http1_0)
    })
    .await;

    transport.write_all("HTTP/1.0 200 Ok\r\nContent-Length: 0\r\n\r\n");
    let conn = conn_fut.await?;
    assert_eq!(Some(Status::Ok), conn.status());

    assert_eq!(
        formatdoc! {"
            GET /path HTTP/1.0\r
            Host: example.com\r
            Accept: */*\r
            User-Agent: {USER_AGENT}\r
            \r
        "},
        transport.read_available_string().await
    );
    Ok(())
}

#[test(harness)]
async fn http1_0_post_no_expect() -> TestResult {
    let (transport, conn_fut) = test_conn(|client| {
        client
            .post("http://example.com/")
            .with_http_version(Version::Http1_0)
            .with_body("hello")
    })
    .await;

    transport.write_all("HTTP/1.0 200 Ok\r\nContent-Length: 0\r\n\r\n");
    let conn = conn_fut.await?;
    assert_eq!(Some(Status::Ok), conn.status());

    // No Expect: 100-continue; Content-Length present; body sent immediately after head
    assert_eq!(
        formatdoc! {"
            POST / HTTP/1.0\r
            Host: example.com\r
            Accept: */*\r
            Content-Length: 5\r
            User-Agent: {USER_AGENT}\r
            \r
            hello\
        "},
        transport.read_available_string().await
    );
    Ok(())
}

#[test(harness)]
async fn http1_0_streaming_body_no_chunked() -> TestResult {
    let (transport, conn_fut) = test_conn(|client| {
        let body = Body::new_streaming(Cursor::new(b"streaming body" as &'static [u8]), None);
        client
            .post("http://example.com/")
            .with_http_version(Version::Http1_0)
            .with_body(body)
    })
    .await;

    transport.write_all("HTTP/1.0 200 Ok\r\nContent-Length: 0\r\n\r\n");
    let conn = conn_fut.await?;
    assert_eq!(Some(Status::Ok), conn.status());

    // No Transfer-Encoding: chunked; no Content-Length; no Expect; raw bytes streamed
    assert_eq!(
        formatdoc! {"
            POST / HTTP/1.0\r
            Host: example.com\r
            Accept: */*\r
            User-Agent: {USER_AGENT}\r
            \r
            streaming body\
        "},
        transport.read_available_string().await
    );
    Ok(())
}

struct TestConnector<R>(Sender<TestTransport>, R);

impl<R: RuntimeTrait> Connector for TestConnector<R> {
    type Runtime = R;
    type Transport = TestTransport;
    type Udp = ();

    async fn connect(&self, _url: &Url) -> io::Result<Self::Transport> {
        let (server, client) = TestTransport::new();
        let _ = self.0.send(server).await;
        Ok(client)
    }

    fn runtime(&self) -> Self::Runtime {
        self.1.clone()
    }

    async fn resolve(&self, _host: &str, _port: u16) -> io::Result<Vec<SocketAddr>> {
        Ok(vec![])
    }
}

async fn test_conn(
    setup: impl FnOnce(Client) -> Conn + Send + 'static,
) -> (TestTransport, impl Future<Output = Result<Conn, Error>>) {
    let (sender, receiver) = async_channel::unbounded();
    let client = Client::new(TestConnector(sender, trillium_testing::runtime()));
    let runtime = client.connector().runtime();
    let conn_fut = runtime.spawn(setup(client).into_future()).into_future();
    let transport = receiver.recv().await.unwrap();
    (transport, async move { conn_fut.await.unwrap() })
}
