use super::*;
use crate::Client;
use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    future::pending,
    pin::Pin,
    sync::Mutex,
    task::{Context, Poll},
    time::Duration,
};
use trillium_http::{
    Headers, Method, Version,
    headers::qpack::{FieldSection, PseudoHeaders},
};
use trillium_server_common::{
    QuicConnectionTrait, QuicTransportBidi, QuicTransportReceive, QuicTransportSend, Transport,
};
use trillium_testing::{TestResult, harness, test};

/// A stream with canned inbound bytes; writes are discarded.
#[derive(Debug, Default)]
struct MockStream {
    input: futures_lite::io::Cursor<Vec<u8>>,
}

impl MockStream {
    fn new(input: Vec<u8>) -> Self {
        Self {
            input: futures_lite::io::Cursor::new(input),
        }
    }
}

impl AsyncRead for MockStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.input).poll_read(cx, buf)
    }
}

impl AsyncWrite for MockStream {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl QuicTransportReceive for MockStream {
    fn stop(&mut self, _code: u64) {}
}

impl QuicTransportSend for MockStream {
    fn reset(&mut self, _code: u64) {}
}

impl QuicTransportBidi for MockStream {}
impl Transport for MockStream {}

#[derive(Clone, Debug, Default)]
struct MockQuic {
    open_uni_fails: bool,
    inbound_bidi: Arc<Mutex<Option<(u64, MockStream)>>>,
    closed_with: Arc<OnceLock<u64>>,
}

impl QuicConnectionTrait for MockQuic {
    type BidiStream = MockStream;
    type RecvStream = MockStream;
    type SendStream = MockStream;

    async fn accept_bidi(&self) -> io::Result<(u64, MockStream)> {
        let stream = self.inbound_bidi.lock().unwrap().take();
        match stream {
            Some(stream) => Ok(stream),
            None => pending().await,
        }
    }

    async fn accept_uni(&self) -> io::Result<(u64, MockStream)> {
        pending().await
    }

    async fn open_uni(&self) -> io::Result<(u64, MockStream)> {
        if self.open_uni_fails {
            Err(io::Error::other("open_uni failed"))
        } else {
            Ok((3, MockStream::default()))
        }
    }

    async fn open_bidi(&self) -> io::Result<(u64, MockStream)> {
        Err(io::Error::other("open_bidi unavailable"))
    }

    fn remote_address(&self) -> SocketAddr {
        ([127, 0, 0, 1], 443).into()
    }

    fn close(&self, error_code: u64, _reason: &[u8]) {
        let _ = self.closed_with.set(error_code);
    }

    fn send_datagram(&self, _data: &[u8]) -> io::Result<()> {
        Err(io::Error::other("datagrams unsupported"))
    }

    async fn recv_datagram<F: FnOnce(&[u8]) + Send>(&self, _callback: F) -> io::Result<()> {
        pending().await
    }

    fn max_datagram_size(&self) -> Option<usize> {
        None
    }
}

/// A GET request as a complete HEADERS frame, encoded by a throwaway connection whose dynamic
/// table has no capacity — so the section is static/literal-only and decodes without any
/// encoder-stream input.
fn encoded_get_request(stream_id: u64) -> Vec<u8> {
    let mut pseudo_headers = PseudoHeaders::default()
        .with_method(Method::Get)
        .with_authority("example.com");
    pseudo_headers.set_path(Some("/")).set_scheme(Some("https"));
    let headers = Headers::new();
    let field_section = FieldSection::new(pseudo_headers, &headers);

    let mut buf = Vec::new();
    H3Connection::new(Arc::new(HttpContext::default()))
        .encode_field_section_framed(&field_section, &mut buf, stream_id)
        .unwrap();
    buf
}

async fn wait_for(runtime: &Runtime, mut predicate: impl FnMut() -> bool) -> bool {
    for _ in 0..500 {
        if predicate() {
            return true;
        }
        runtime.delay(Duration::from_millis(1)).await;
    }
    predicate()
}

#[test(harness)]
async fn open_uni_failure_during_setup_marks_the_connection_dead() {
    let runtime = Runtime::new(trillium_testing::runtime());
    let context = Arc::new(HttpContext::default());
    let mock = MockQuic {
        open_uni_fails: true,
        ..Default::default()
    };
    let entry = setup_h3_connection(mock.into(), &context, &runtime);

    assert!(
        wait_for(&runtime, || matches!(
            entry.classify(),
            PoolEntryStatus::Dead
        ))
        .await,
        "open_uni failure during setup must not leave the pooled connection available"
    );
}

#[test(harness)]
async fn server_initiated_request_stream_closes_the_connection() {
    let runtime = Runtime::new(trillium_testing::runtime());
    let context = Arc::new(HttpContext::default());
    let mock = MockQuic {
        inbound_bidi: Arc::new(Mutex::new(Some((
            1,
            MockStream::new(encoded_get_request(1)),
        )))),
        ..Default::default()
    };
    let closed_with = mock.closed_with.clone();
    let entry = setup_h3_connection(mock.into(), &context, &runtime);

    assert!(
        wait_for(&runtime, || matches!(
            entry.classify(),
            PoolEntryStatus::Dead
        ))
        .await,
        "a server-initiated request stream must kill the pooled connection"
    );
    assert_eq!(
        closed_with.get(),
        Some(&u64::from(H3ErrorCode::StreamCreationError)),
        "the connection must close with H3_STREAM_CREATION_ERROR"
    );
}

#[test(harness)]
async fn h3_idle_timeout_expires_pooled_connections() -> TestResult {
    let rcgen::CertifiedKey {
        cert, signing_key, ..
    } = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let (cert_pem, key_pem) = (
        cert.pem().into_bytes(),
        signing_key.serialize_pem().into_bytes(),
    );

    let rustls_client_config = || {
        let mut roots = trillium_rustls::rustls::RootCertStore::empty();
        roots.add(cert.der().clone()).unwrap();
        trillium_rustls::rustls::ClientConfig::builder_with_provider(
            trillium_rustls::rustls::crypto::aws_lc_rs::default_provider().into(),
        )
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth()
    };

    let server = trillium_smol::config()
        .with_host("localhost")
        .with_port(0)
        .with_acceptor(trillium_rustls::RustlsAcceptor::from_single_cert(
            &cert_pem, &key_pem,
        ))
        .with_quic(trillium_quinn::QuicConfig::from_single_cert(
            &cert_pem, &key_pem,
        ))
        .spawn(|conn: trillium::Conn| async move { conn.ok("hello") });
    let port = server.info().await.tcp_socket_addr().unwrap().port();

    let client = Client::new_with_quic(
        trillium_rustls::RustlsConfig::new(
            rustls_client_config(),
            trillium_smol::ClientConfig::default(),
        ),
        trillium_quinn::ClientQuicConfig::from_rustls_client_config(rustls_client_config()),
    )
    .with_h3_idle_timeout(Duration::from_millis(10))
    .with_base(format!("https://localhost:{port}"));

    let conn = client.get("/").with_http_version(Version::Http3).await?;
    assert_eq!(conn.http_version(), Version::Http3);
    drop(conn);

    let pool = &client.h3().unwrap().pool;
    assert_eq!(pool.keys().count(), 1);

    let runtime = Runtime::new(trillium_testing::runtime());
    runtime.delay(Duration::from_millis(20)).await;
    pool.reap();
    assert_eq!(
        pool.keys().count(),
        0,
        "an h3 connection past its idle timeout should be reaped"
    );

    server.shut_down().await;
    Ok(())
}
