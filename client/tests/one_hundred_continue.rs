use async_channel::Sender;
use futures_lite::future;
use indoc::{formatdoc, indoc};
use pretty_assertions::assert_eq;
use std::future::{Future, IntoFuture};
use test_harness::test;
use trillium_client::{Client, Conn, Error, Status, USER_AGENT};
use trillium_server_common::{Connector, Url};
use trillium_testing::{RuntimeTrait, TestResult, TestTransport, harness};

#[test(harness)]
async fn extra_one_hundred_continue() -> TestResult {
    let (transport, conn_fut) =
        test_conn(|client| client.post("http://example.com").with_body("body")).await;

    let expected_request_head = formatdoc! {"
        POST / HTTP/1.1\r
        Host: example.com\r
        Accept: */*\r
        Connection: close\r
        Content-Length: 4\r
        Expect: 100-continue\r
        User-Agent: {USER_AGENT}\r
        \r
    "};

    assert_eq!(
        expected_request_head,
        transport.read_available_string().await
    );

    transport.write_all("HTTP/1.1 100 Continue\r\n\r\n");
    assert_eq!("body", transport.read_available_string().await);

    transport.write_all("HTTP/1.1 100 Continue\r\nServer: Caddy\r\n\r\n"); //<-

    let response_head = formatdoc! {"
        HTTP/1.1 200 Ok\r
        Date: {TEST_DATE}\r
        Connection: close\r
        Content-Length: 20\r
        Server: text\r
        \r
        response: 0123456789\
    "};

    transport.write_all(response_head);

    let mut conn = conn_fut.await.unwrap();
    assert_eq!(
        "response: 0123456789",
        conn.response_body().read_string().await?
    );

    assert_eq!(
        conn.response_headers().get_values("Server").unwrap(),
        ["Caddy", "text"].as_slice()
    );

    assert_eq!(Some(Status::Ok), conn.status());

    Ok(())
}

#[test(harness)]
async fn one_hundred_continue() -> TestResult {
    let (transport, conn_fut) =
        test_conn(|client| client.post("http://example.com").with_body("body")).await;

    let expected_request = formatdoc! {"
        POST / HTTP/1.1\r
        Host: example.com\r
        Accept: */*\r
        Connection: close\r
        Content-Length: 4\r
        Expect: 100-continue\r
        User-Agent: {USER_AGENT}\r
        \r
    "};

    assert_eq!(expected_request, transport.read_available_string().await);

    transport.write_all("HTTP/1.1 100 Continue\r\n\r\n");
    assert_eq!("body", transport.read_available_string().await);

    transport.write_all(formatdoc! {"
        HTTP/1.1 200 Ok\r
        Date: {TEST_DATE}\r
        Accept: */*\r
        Connection: close\r
        Content-Length: 20\r
        Server: text\r
        \r
        response: 0123456789\
    "});

    let mut conn = conn_fut.await.unwrap();

    assert_eq!(
        "response: 0123456789",
        conn.response_body().read_string().await?
    );

    assert_eq!(Some(Status::Ok), conn.status());

    Ok(())
}

#[test(harness)]
async fn empty_body_no_100_continue() -> TestResult {
    let (transport, conn_fut) =
        test_conn(|client| client.post("http://example.com").with_body("")).await;

    let expected_request = formatdoc! {"
        POST / HTTP/1.1\r
        Host: example.com\r
        Accept: */*\r
        Connection: close\r
        User-Agent: {USER_AGENT}\r
        \r
    "};

    assert_eq!(expected_request, transport.read_available_string().await);

    transport.write_all(formatdoc! {"
        HTTP/1.1 200 Ok\r
        Date: {TEST_DATE}\r
        Connection: close\r
        Content-Length: 20\r
        Server: text\r
        \r
        response: 0123456789\
    "});

    let conn = conn_fut.await.unwrap();
    assert_eq!(Some(Status::Ok), conn.status());
    Ok(())
}

#[test(harness)]
async fn two_small_continues() -> TestResult {
    let (transport, conn_fut) =
        test_conn(|client| client.post("http://example.com").with_body("body")).await;
    let expected_request = formatdoc! {"
        POST / HTTP/1.1\r
        Host: example.com\r
        Accept: */*\r
        Connection: close\r
        Content-Length: 4\r
        Expect: 100-continue\r
        User-Agent: {USER_AGENT}\r
        \r
    "};

    assert_eq!(expected_request, transport.read_available_string().await);

    for _ in 0..2 {
        transport.write_all("HTTP/1.1 100 Continue\r\n");
        future::yield_now().await;
        transport.write_all("\r\n");
        future::yield_now().await;
    }
    assert_eq!("body", transport.read_available_string().await);

    transport.write_all(formatdoc! {"
        HTTP/1.1 200 Ok\r
        Date: {TEST_DATE}\r
        Connection: close\r
        Content-Length: 0\r
        \r
    "});
    let conn = conn_fut.await.unwrap();
    assert_eq!(Some(Status::Ok), conn.status());

    Ok(())
}

#[test(harness)]
async fn little_continue_big_continue() -> TestResult {
    let (transport, conn_fut) =
        test_conn(|client| client.post("http://example.com").with_body("body")).await;

    let expected_request = formatdoc! {"
        POST / HTTP/1.1\r
        Host: example.com\r
        Accept: */*\r
        Connection: close\r
        Content-Length: 4\r
        Expect: 100-continue\r
        User-Agent: {USER_AGENT}\r
        \r
    "};

    assert_eq!(expected_request, transport.read_available_string().await);

    transport.write_all(indoc! {"
        HTTP/1.1 100 Continue\r
        \r
        HTTP/1.1 100 Continue\r
        X-Filler: AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\r
        \r
    "});
    assert_eq!("body", transport.read_available_string().await);

    transport.write_all(formatdoc! {"
        HTTP/1.1 200 Ok\r
        Date: {TEST_DATE}\r
        Connection: close\r
        Content-Length: 0\r
        \r
    "});
    let conn = conn_fut.await.unwrap();
    assert_eq!(Some(Status::Ok), conn.status());
    Ok(())
}

const TEST_DATE: &str = "Tue, 21 Nov 2023 21:27:21 GMT";

struct TestConnector<R>(Sender<TestTransport>, R);

impl<R: RuntimeTrait> Connector for TestConnector<R> {
    type Runtime = R;
    type Transport = TestTransport;

    async fn connect(&self, _url: &Url) -> std::io::Result<Self::Transport> {
        let (server, client) = TestTransport::new();
        let _ = self.0.send(server).await;
        Ok(client)
    }

    fn runtime(&self) -> Self::Runtime {
        self.1.clone()
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
