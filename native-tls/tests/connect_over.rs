use rcgen::generate_simple_self_signed;
use std::sync::Arc;
use trillium_native_tls::{NativeTlsConfig, async_native_tls, native_tls};
use trillium_rustls::RustlsAcceptor;
use trillium_server_common::{Connector, Destination, Transport};
use trillium_smol::{ClientConfig, config};
use trillium_testing::{
    TestTransport,
    futures_lite::{AsyncReadExt, AsyncWriteExt},
    harness, test,
};

#[test(harness)]
async fn connect_over_validates_destination_identity() {
    let cert =
        generate_simple_self_signed(["origin.example".to_owned(), "127.0.0.2".to_owned()]).unwrap();
    let pem = cert.cert.pem();
    let key = cert.signing_key.serialize_pem();
    let port = portpicker::pick_unused_port().unwrap();
    let handle = config()
        .with_host("127.0.0.1")
        .with_port(port)
        .with_acceptor(RustlsAcceptor::from_single_cert(
            pem.as_bytes(),
            key.as_bytes(),
        ))
        .spawn("ok");
    handle.info().await;

    let tcp = ClientConfig::default();
    let connector = NativeTlsConfig {
        tls_connector: Arc::new(
            async_native_tls::TlsConnector::new()
                .add_root_certificate(native_tls::Certificate::from_pem(pem.as_bytes()).unwrap()),
        ),
        tcp_config: tcp,
    };
    let dial_dest =
        Destination::new_with_socket_addrs(false, [format!("127.0.0.1:{port}").parse().unwrap()]);

    for (destination, succeeds) in [
        (
            Destination::new_with_host(true, "origin.example", port),
            true,
        ),
        (
            Destination::new_with_host(true, "wrong.example", port),
            false,
        ),
        (
            Destination::new_with_socket_addrs(
                true,
                [format!("127.0.0.2:{port}").parse().unwrap()],
            ),
            true,
        ),
        (
            Destination::new_with_socket_addrs(
                true,
                [format!("127.0.0.1:{port}").parse().unwrap()],
            ),
            false,
        ),
    ] {
        let raw: Box<dyn Transport> = Box::new(tcp.connect_to(dial_dest.clone()).await.unwrap());
        let result = connector.connect_over(raw, destination).await;
        assert_eq!(result.is_ok(), succeeds, "{:?}", result.as_ref().err());
        if let Ok(mut stream) = result {
            stream
                .write_all(b"GET / HTTP/1.1\r\nHost: origin.example\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).await.unwrap();
            assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response:?}");
            assert!(response.ends_with("\r\n\r\nok"), "{response:?}");
        }
    }

    handle.shut_down().await;
}

#[test(harness)]
async fn connect_over_non_secure_passes_through_without_tls() {
    let connector = NativeTlsConfig::<ClientConfig>::default();
    let (client_end, mut server_end) = TestTransport::new();
    let mut wrapped = connector
        .connect_over(
            Box::new(client_end),
            Destination::new_with_host(false, "origin.example", 80),
        )
        .await
        .unwrap();

    wrapped.write_all(b"plain-bytes").await.unwrap();
    let mut buf = [0u8; 11];
    server_end.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"plain-bytes");
}
