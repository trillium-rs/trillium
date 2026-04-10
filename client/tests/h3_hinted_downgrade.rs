use trillium_client::{Client, Version};
use trillium_quinn::ClientQuicConfig;
use trillium_testing::{harness, test};

#[test(harness)]
async fn h3_hinted_downgrades() {
    let server = trillium_smol::config()
        .with_port(0)
        .spawn(|conn: trillium::Conn| async move {
            let version = conn.http_version();
            conn.ok(version.as_str())
        });

    let socket_addr = server.info().await.tcp_socket_addr().copied().unwrap();

    let client = Client::new_with_quic(
        trillium_smol::ClientConfig::default(),
        ClientQuicConfig::with_webpki_roots(),
    )
    .with_base(socket_addr);

    let mut conn = client
        .get("/")
        .with_http_version(Version::Http3)
        .await
        .unwrap();
    assert_eq!(conn.response_body().await.unwrap(), "HTTP/1.1");
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http1_1);
    server.shut_down().await;
}
