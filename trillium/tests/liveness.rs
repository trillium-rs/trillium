use futures_lite::{future::poll_once, AsyncReadExt, AsyncWriteExt};
use std::future::pending;
use test_harness::test;
use trillium::Conn;
use trillium_testing::{config, harness, ClientConfig, Connector, ObjectSafeConnector, TestResult};

#[test(harness)]
async fn infinitely_pending_task() -> TestResult {
    let handle = config()
        .with_host("localhost")
        .with_port(0)
        .spawn(|mut conn: Conn| async move {
            conn.cancel_on_disconnect(pending::<()>()).await;
            conn
        });

    let info = handle.info().await;

    let url = format!("http://{}", info.listener_description())
        .parse()
        .unwrap();
    let mut client = Connector::connect(&ClientConfig::default().boxed(), &url).await?;

    client
        .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n")
        .await?;

    let mut byte = [0u8];
    assert!(poll_once(client.read(&mut byte)).await.is_none()); // nothing to read; the handler has
                                                                // not responded

    client.close().await?; // closing the client before we receive a response

    handle.stop().await; // wait for a graceful shutdown the fact that we terminate here indicates
                         // that the handler is not still running even though it polls an infinitely
                         // pending future

    Ok(())
}
