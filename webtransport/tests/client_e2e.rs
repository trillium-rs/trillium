//! End-to-end tests of `trillium_client::Client::webtransport(...)` against the
//! `trillium_webtransport::WebTransport` server handler.

use futures_lite::{AsyncReadExt, AsyncWriteExt};
use rcgen::generate_simple_self_signed;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use trillium::Handler;
use trillium_client::Client;
use trillium_quinn::{ClientQuicConfig, QuicConfig};
use trillium_rustls::RustlsConfig;
use trillium_tokio::ClientConfig;
use trillium_webtransport::{InboundStream, WebTransport, WebTransportConnection};

struct TestCert {
    cert_pem: Vec<u8>,
    key_pem: Vec<u8>,
    cert_der: rustls::pki_types::CertificateDer<'static>,
}

fn test_cert() -> TestCert {
    let rcgen::CertifiedKey {
        cert, signing_key, ..
    } = generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    TestCert {
        cert_pem: cert.pem().into_bytes(),
        key_pem: signing_key.serialize_pem().into_bytes(),
        cert_der: cert.der().clone(),
    }
}

fn rustls_client_config(tc: &TestCert) -> rustls::ClientConfig {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.add(tc.cert_der.clone()).unwrap();
    rustls::ClientConfig::builder_with_provider(
        rustls::crypto::aws_lc_rs::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(root_store)
    .with_no_client_auth()
}

async fn start_server(handler: impl Handler, tc: &TestCert) -> SocketAddr {
    let handle = trillium_tokio::config()
        .with_port(0)
        .with_host("localhost")
        .without_signals()
        .with_acceptor(trillium_rustls::RustlsAcceptor::from_single_cert(
            &tc.cert_pem,
            &tc.key_pem,
        ))
        .with_quic(QuicConfig::from_single_cert(&tc.cert_pem, &tc.key_pem))
        .spawn(handler);
    *handle.info().await.tcp_socket_addr().unwrap()
}

fn trillium_client(tc: &TestCert) -> Client {
    Client::new_with_quic(
        RustlsConfig::new(rustls_client_config(tc), ClientConfig::default()),
        ClientQuicConfig::from_rustls_client_config(rustls_client_config(tc)),
    )
}

/// A WT handler that handles one client-initiated bidi stream as an uppercase echo and
/// echoes one datagram. Counts how many distinct QUIC connections it has seen via
/// `peer_addr().port()`.
async fn echo_handler(wt: WebTransportConnection) {
    let datagram = async {
        if let Some(d) = wt.recv_datagram().await {
            let _ = wt.send_datagram(&d);
        }
    };

    let stream = async {
        if let Some(InboundStream::Bidi(mut s)) = wt.accept_next_stream().await {
            let mut buf = Vec::new();
            if s.read_to_end(&mut buf).await.is_ok_and(|n| n > 0) {
                let response = String::from_utf8_lossy(&buf).to_uppercase();
                let _ = s.write_all(response.as_bytes()).await;
                let _ = s.close().await;
            }
        }
    };

    futures_lite::future::zip(datagram, stream).await;
}

#[tokio::test]
async fn client_webtransport_round_trip() {
    let tc = test_cert();
    let addr = start_server(WebTransport::new(echo_handler), &tc).await;

    let client = trillium_client(&tc);
    let url = format!("https://localhost:{}/wt", addr.port());

    let wt = client
        .webtransport(url.as_str())
        .into_webtransport()
        .await
        .expect("webtransport upgrade should succeed");

    // Open a bidi from the client, send "hello", read uppercased echo back.
    let mut stream = wt.open_bidi().await.unwrap();
    stream.write_all(b"hello").await.unwrap();
    stream.close().await.unwrap();

    let mut received = Vec::new();
    stream.read_to_end(&mut received).await.unwrap();
    assert_eq!(&received, b"HELLO");

    // Send a datagram, expect an echo back.
    wt.send_datagram(b"ping").unwrap();
    let echoed = tokio::time::timeout(Duration::from_secs(2), wt.recv_datagram())
        .await
        .expect("datagram should arrive within 2s")
        .expect("datagram channel should not close");
    assert_eq!(&*echoed, b"ping");
}

/// Counts how many sessions the WT handler has handled, and asserts each saw the same peer
/// address (proving the same QUIC connection was reused).
#[derive(Default, Clone)]
struct MultiplexProbe {
    seen_peers: Arc<std::sync::Mutex<Vec<SocketAddr>>>,
}

impl MultiplexProbe {
    async fn handle(self, wt: WebTransportConnection) {
        self.seen_peers.lock().unwrap().push(wt.peer_addr());
        // Echo the bidi stream so the client can verify the session is alive.
        if let Some(InboundStream::Bidi(mut s)) = wt.accept_next_stream().await {
            let mut buf = Vec::new();
            if s.read_to_end(&mut buf).await.is_ok_and(|n| n > 0) {
                let _ = s.write_all(&buf).await;
                let _ = s.close().await;
            }
        }
    }
}

#[tokio::test]
async fn multiple_webtransport_sessions_share_quic_connection() {
    let tc = test_cert();
    let probe = MultiplexProbe::default();
    let probe_for_handler = probe.clone();

    let handler = WebTransport::new(move |wt: WebTransportConnection| {
        let probe = probe_for_handler.clone();
        async move { probe.handle(wt).await }
    });

    let addr = start_server(handler, &tc).await;

    let client = trillium_client(&tc);
    let url = format!("https://localhost:{}/multiplex", addr.port());

    // Open three sessions to the same origin in sequence.
    for i in 0..3 {
        let wt = client
            .webtransport(url.as_str())
            .into_webtransport()
            .await
            .unwrap_or_else(|e| panic!("session {i} upgrade failed: {e}"));

        let mut stream = wt.open_bidi().await.unwrap();
        let payload = format!("session-{i}");
        stream.write_all(payload.as_bytes()).await.unwrap();
        stream.close().await.unwrap();

        let mut received = Vec::new();
        stream.read_to_end(&mut received).await.unwrap();
        assert_eq!(received, payload.as_bytes());
    }

    // Give the server's accept loop a moment to register all three peer-addr observations.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let peers = probe.seen_peers.lock().unwrap().clone();
    assert_eq!(peers.len(), 3, "server should have seen 3 sessions");
    let first = peers[0];
    assert!(
        peers.iter().all(|p| *p == first),
        "all sessions must share the same QUIC connection (peer_addr); got: {peers:?}"
    );
}
