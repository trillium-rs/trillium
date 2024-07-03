use async_channel::Sender;
use test_harness::test;
use trillium::{Conn, Handler, Status};
use trillium_testing::{
    client_config, futures_lite::AsyncWriteExt, harness, ArcedConnector, AsyncWrite, Connector,
    ServerHandle,
};
use url::Url;

struct LastStatus(Sender<Option<Status>>);

impl Handler for LastStatus {
    async fn run(&self, conn: Conn) -> Conn {
        conn
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        self.0.send(conn.status()).await.unwrap();
        conn
    }
}

#[test(harness)]
async fn disconnect_on_string_body() {
    async fn api_handler(conn: &mut Conn, body: String) {
        conn.set_body(body);
    }
    let (sender, receiver) = async_channel::bounded(1);
    let handler = (LastStatus(sender), trillium_api::api(api_handler));
    let (handle, mut client) = establish_server(handler).await;

    client
        .write_all(b"POST / HTTP/1.1\r\nHost: example.com\r\nContent-Length: 10\r\n\r\nnot ten")
        .await
        .unwrap();

    drop(client);
    assert_eq!(Some(Status::BadRequest), receiver.recv().await.unwrap());
    handle.shut_down().await;
}

/// this test exists to confirm that the 400 response tested above is in fact due to the disconnect
#[test(harness)]
async fn normal_string_body() {
    async fn api_handler(conn: &mut Conn, body: String) {
        conn.set_body(body);
    }
    let (sender, receiver) = async_channel::bounded(1);
    let handler = (LastStatus(sender), trillium_api::api(api_handler));
    let (handle, mut client) = establish_server(handler).await;
    client
        .write_all(b"POST / HTTP/1.1\r\nHost: example.com\r\nContent-Length: 10\r\n\r\nexactlyten")
        .await
        .unwrap();

    drop(client);
    assert_eq!(Some(Status::Ok), receiver.recv().await.unwrap());
    handle.shut_down().await;
}

#[test(harness)]
async fn disconnect_on_vec_body() {
    async fn api_handler(conn: &mut Conn, body: Vec<u8>) {
        conn.set_body(body);
    }
    let (sender, receiver) = async_channel::bounded(1);
    let handler = (LastStatus(sender), trillium_api::api(api_handler));
    let (handle, mut client) = establish_server(handler).await;

    client
        .write_all(b"POST / HTTP/1.1\r\nHost: example.com\r\nContent-Length: 10\r\n\r\nnot ten")
        .await
        .unwrap();

    drop(client);
    assert_eq!(Some(Status::BadRequest), receiver.recv().await.unwrap());
    handle.shut_down().await;
}

/// this test exists to confirm that the 400 response tested above is in fact due to the disconnect
#[test(harness)]
async fn normal_vec_body() {
    async fn api_handler(conn: &mut Conn, body: Vec<u8>) {
        conn.set_body(body);
    }
    let (sender, receiver) = async_channel::bounded(1);
    let handler = (LastStatus(sender), trillium_api::api(api_handler));
    let (handle, mut client) = establish_server(handler).await;
    client
        .write_all(b"POST / HTTP/1.1\r\nHost: example.com\r\nContent-Length: 10\r\n\r\nexactlyten")
        .await
        .unwrap();

    drop(client);
    assert_eq!(Some(Status::Ok), receiver.recv().await.unwrap());
    handle.shut_down().await;
}

async fn establish_server(handler: impl Handler) -> (ServerHandle, impl AsyncWrite) {
    let _ = env_logger::builder().is_test(true).try_init();

    let handle = trillium_testing::config().with_port(0).spawn(handler);
    let info = handle.info().await;
    let url: &Url = info.state().unwrap();

    let client = ArcedConnector::new(client_config())
        .connect(url)
        .await
        .unwrap();
    (handle, client)
}
