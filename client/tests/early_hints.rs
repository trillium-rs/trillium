use async_channel::Sender;
use indoc::{formatdoc, indoc};
use pretty_assertions::assert_eq;
use std::{
    future::{Future, IntoFuture},
    io,
    net::SocketAddr,
};
use test_harness::test;
use trillium_client::{Client, Conn, Error, KnownHeaderName, Status, USER_AGENT};
use trillium_server_common::{Connector, Url};
use trillium_testing::{RuntimeTrait, TestResult, TestTransport, harness};

#[test(harness)]
async fn early_hints_then_final() -> TestResult {
    let (transport, conn_fut) = test_conn(|client| client.get("http://example.com")).await;

    let expected_request = formatdoc! {"
        GET / HTTP/1.1\r
        Host: example.com\r
        Accept: */*\r
        User-Agent: {USER_AGENT}\r
        \r
    "};

    assert_eq!(expected_request, transport.read_available_string().await);

    // 103 Early Hints with link headers. Per RFC 8297 §2 these must be treated as
    // informational only — the recipient cannot rely on them being included in the
    // final response and they MUST NOT be merged into final response headers.
    transport.write_all(indoc! {"
        HTTP/1.1 103 Early Hints\r
        Link: </styles.css>; rel=preload; as=style\r
        Link: </script.js>; rel=preload; as=script\r
        \r
    "});

    transport.write_all(formatdoc! {"
        HTTP/1.1 200 Ok\r
        Date: {TEST_DATE}\r
        Connection: close\r
        Content-Length: 5\r
        Server: text\r
        \r
        hello\
    "});

    let mut conn = conn_fut.await.unwrap();
    assert_eq!("hello", conn.response_body().read_string().await?);

    assert_eq!(Some(Status::Ok), conn.status());
    assert_eq!(
        conn.response_headers().get_str(KnownHeaderName::Server),
        Some("text")
    );
    assert!(conn.response_headers().get(KnownHeaderName::Link).is_none());

    Ok(())
}

#[test(harness)]
async fn multiple_early_hints_then_final() -> TestResult {
    let (transport, conn_fut) = test_conn(|client| client.get("http://example.com")).await;

    let expected_request = formatdoc! {"
        GET / HTTP/1.1\r
        Host: example.com\r
        Accept: */*\r
        User-Agent: {USER_AGENT}\r
        \r
    "};

    assert_eq!(expected_request, transport.read_available_string().await);

    // RFC 8297 §2: a server may send any number of 103 responses prior to the final.
    // Headers from every one of them must be discarded by the client.
    transport.write_all(indoc! {"
        HTTP/1.1 103 Early Hints\r
        Link: </styles.css>; rel=preload; as=style\r
        \r
        HTTP/1.1 103 Early Hints\r
        Link: </script.js>; rel=preload; as=script\r
        X-Hint: something\r
        \r
    "});

    transport.write_all(formatdoc! {"
        HTTP/1.1 200 Ok\r
        Date: {TEST_DATE}\r
        Connection: close\r
        Content-Length: 5\r
        Server: text\r
        \r
        hello\
    "});

    let mut conn = conn_fut.await.unwrap();
    assert_eq!("hello", conn.response_body().read_string().await?);

    assert_eq!(Some(Status::Ok), conn.status());
    assert!(conn.response_headers().get(KnownHeaderName::Link).is_none());
    assert!(conn.response_headers().get("X-Hint").is_none());

    Ok(())
}

#[test(harness)]
async fn early_hints_then_continue_then_final() -> TestResult {
    // POST with a body — a zero buffer threshold forces `Expect: 100-continue` even for this
    // tiny body (a body above the threshold would trigger it naturally). The server sends 103
    // first (early hints before granting the body), then 100 (granting), then expects the body,
    // then sends the final 200. The client should:
    //   - tolerate the 103 while still in the pre-body Expect-100 wait
    //   - discard its headers
    //   - proceed to send the body once 100 arrives
    //   - return a final response carrying neither the 103's nor the 100's headers.
    let (transport, conn_fut) = test_conn(|client| {
        client
            .with_max_buffered_request_body(0)
            .post("http://example.com")
            .with_body("body")
    })
    .await;

    let expected_request_head = formatdoc! {"
        POST / HTTP/1.1\r
        Host: example.com\r
        Accept: */*\r
        Content-Length: 4\r
        Expect: 100-continue\r
        User-Agent: {USER_AGENT}\r
        \r
    "};

    assert_eq!(
        expected_request_head,
        transport.read_available_string().await
    );

    transport.write_all(indoc! {"
        HTTP/1.1 103 Early Hints\r
        Link: </styles.css>; rel=preload; as=style\r
        \r
        HTTP/1.1 100 Continue\r
        Server: Caddy\r
        \r
    "});

    assert_eq!("body", transport.read_available_string().await);

    transport.write_all(formatdoc! {"
        HTTP/1.1 200 Ok\r
        Date: {TEST_DATE}\r
        Connection: close\r
        Content-Length: 5\r
        Server: text\r
        \r
        hello\
    "});

    let mut conn = conn_fut.await.unwrap();
    assert_eq!("hello", conn.response_body().read_string().await?);

    assert_eq!(Some(Status::Ok), conn.status());
    assert_eq!(
        conn.response_headers()
            .get_values(KnownHeaderName::Server)
            .unwrap(),
        ["text"].as_slice()
    );
    assert!(conn.response_headers().get(KnownHeaderName::Link).is_none());

    Ok(())
}

const TEST_DATE: &str = "Tue, 21 Nov 2023 21:27:21 GMT";

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
