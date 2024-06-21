use futures_lite::{future::poll_once, AsyncRead, AsyncReadExt, AsyncWriteExt};
use std::{
    future::{pending, Future},
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use test_harness::test;
use trillium::Conn;
use trillium_testing::{client_config, config, harness, ArcedConnector, Connector, TestResult};

#[test(harness)]
async fn infinitely_pending_task() -> TestResult {
    let connector = ArcedConnector::new(client_config());

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
    let mut client = connector.connect(&url).await?;

    client
        .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n")
        .await?;

    let mut byte = [0u8];
    assert!(poll_once(client.read(&mut byte)).await.is_none()); // nothing to read; the handler has
    // not responded

    client.close().await?; // closing the client before we receive a response

    handle.shut_down().await; // wait for a graceful shutdown the fact that we terminate here indicates
    // that the handler is not still running even though it polls an infinitely
    // pending future

    Ok(())
}

#[test(harness)]
async fn is_disconnected() -> TestResult {
    let _ = env_logger::builder().is_test(true).try_init();
    let connector = ArcedConnector::new(client_config());
    let (delay_sender, delay_receiver) = async_channel::unbounded();
    let (disconnected_sender, disconnected_receiver) = async_channel::unbounded();
    let handle = config()
        .with_host("localhost")
        .with_port(0)
        .spawn(move |mut conn: Conn| {
            let disconnected_sender = disconnected_sender.clone();
            let delay_receiver = delay_receiver.clone();
            async move {
                delay_receiver.recv().await.unwrap();
                disconnected_sender
                    .send(dbg!(conn.is_disconnected().await))
                    .await
                    .unwrap();
                conn.ok("ok")
            }
        });

    let info = handle.info().await;
    let runtime = handle.runtime();

    let url = format!("http://{}", info.listener_description())
        .parse()
        .unwrap();
    let mut client = connector.connect(&url).await?;

    client
        .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n")
        .await?;

    delay_sender.send(()).await?;

    assert!(!disconnected_receiver.recv().await?);

    let s = String::from_utf8(ReadAvailable(&mut client).await?)?;
    assert!(s.starts_with("HTTP/1.1 200 OK\r\n"));
    client.close().await?;

    let mut client = connector.connect(&url).await?;
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n")
        .await?;
    drop(client);
    runtime.delay(Duration::from_millis(10)).await;
    delay_sender.send(()).await?;
    assert!(disconnected_receiver.recv().await?);

    handle.shut_down().await;

    Ok(())
}

struct ReadAvailable<T>(T);
impl<T: AsyncRead + Unpin> Future for ReadAvailable<T> {
    type Output = io::Result<Vec<u8>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut buf = vec![];
        let mut bytes_read = 0;
        loop {
            if buf.len() == bytes_read {
                buf.reserve(32);
                buf.resize(buf.capacity(), 0);
            }
            match Pin::new(&mut self.0).poll_read(cx, &mut buf[bytes_read..]) {
                Poll::Ready(Ok(0)) => break,
                Poll::Ready(Ok(new_bytes)) => {
                    bytes_read += new_bytes;
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending if bytes_read == 0 => return Poll::Pending,
                Poll::Pending => break,
            }
        }

        buf.truncate(bytes_read);
        Poll::Ready(Ok(buf))
    }
}
