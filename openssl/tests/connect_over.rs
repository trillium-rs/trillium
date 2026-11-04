use openssl::{
    ssl::{SslConnector, SslMethod},
    x509::X509,
};
use rcgen::generate_simple_self_signed;
use std::borrow::Cow;
use trillium_openssl::OpenSslConfig;
use trillium_rustls::RustlsAcceptor;
use trillium_server_common::{Connector, Destination, Transport};
use trillium_smol::{ClientConfig, config};
use trillium_testing::{
    TestTransport,
    futures_lite::{AsyncReadExt, AsyncWriteExt},
    harness, test,
};

fn connector_with_root_pem(pem: &[u8]) -> OpenSslConfig<ClientConfig> {
    let mut builder = SslConnector::builder(SslMethod::tls_client()).unwrap();
    builder
        .cert_store_mut()
        .add_cert(X509::from_pem(pem).unwrap())
        .unwrap();
    OpenSslConfig::new(builder.build(), ClientConfig::default())
}

#[test(harness)]
async fn connect_over_validates_destination_identity_and_alpn() {
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
    let connector = connector_with_root_pem(pem.as_bytes());
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
        let destination = destination.with_alpn([Cow::Borrowed(b"http/1.1".as_slice())]);
        let result = connector.connect_over(raw, destination).await;
        assert_eq!(result.is_ok(), succeeds, "{:?}", result.as_ref().err());
        if let Ok(mut stream) = result {
            assert_eq!(
                stream.negotiated_alpn().as_deref(),
                Some(b"http/1.1".as_slice())
            );
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

    // Also verify connect_to with a bare-IP destination (validates against the dialed peer IP).
    let result = connector
        .connect_to(Destination::new_with_socket_addrs(
            true,
            [format!("127.0.0.1:{port}").parse().unwrap()],
        ))
        .await;
    assert!(result.is_err());

    handle.shut_down().await;

    let ip_cert = generate_simple_self_signed(["127.0.0.1".to_owned()]).unwrap();
    let ip_pem = ip_cert.cert.pem();
    let ip_key = ip_cert.signing_key.serialize_pem();
    let ip_port = portpicker::pick_unused_port().unwrap();
    let ip_handle = config()
        .with_host("127.0.0.1")
        .with_port(ip_port)
        .with_acceptor(RustlsAcceptor::from_single_cert(
            ip_pem.as_bytes(),
            ip_key.as_bytes(),
        ))
        .spawn("ok");
    ip_handle.info().await;

    let ip_connector = connector_with_root_pem(ip_pem.as_bytes());
    let mut stream = ip_connector
        .connect_to(
            Destination::new_with_socket_addrs(
                true,
                [format!("127.0.0.1:{ip_port}").parse().unwrap()],
            )
            .with_alpn([Cow::Borrowed(b"http/1.1".as_slice())]),
        )
        .await
        .unwrap();
    assert_eq!(
        stream.negotiated_alpn().as_deref(),
        Some(b"http/1.1".as_slice())
    );
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response:?}");

    ip_handle.shut_down().await;
}

#[test(harness)]
async fn connect_over_non_secure_passes_through_without_tls() {
    let connector = OpenSslConfig::<ClientConfig>::default();
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
