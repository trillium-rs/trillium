use rcgen::generate_simple_self_signed;
use trillium::{Conn, KnownHeaderName};
use trillium_client::{Client, Proxy as ClientProxy};
use trillium_proxy::{ForwardProxyConnect, Proxy, upstream::ForwardProxy};
use trillium_rustls::{RustlsAcceptor, RustlsClientConfig, RustlsConfig};
use trillium_smol::ClientConfig;
use trillium_testing::{harness, test};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct CertPair {
    cert_pem: Vec<u8>,
    key_pem: Vec<u8>,
}

impl CertPair {
    fn generate(names: &[&str]) -> Self {
        let certified = generate_simple_self_signed(
            names
                .iter()
                .map(|&name| name.to_owned())
                .collect::<Vec<_>>(),
        )
        .expect("generate self-signed certificate");
        Self {
            cert_pem: certified.cert.pem().into_bytes(),
            key_pem: certified.signing_key.serialize_pem().into_bytes(),
        }
    }

    fn acceptor(&self) -> RustlsAcceptor {
        RustlsAcceptor::from_single_cert(&self.cert_pem, &self.key_pem)
    }
}

fn tls_client(certs: &[&CertPair]) -> Client {
    let mut pem = Vec::new();
    for cert in certs {
        pem.extend_from_slice(&cert.cert_pem);
    }
    let rustls_config = RustlsClientConfig::from_root_cert_pem(&pem).expect("valid root cert pem");
    Client::new(RustlsConfig::new(rustls_config, ClientConfig::default()))
}

async fn origin(mut conn: Conn) -> Conn {
    assert!(
        conn.request_headers()
            .get(KnownHeaderName::ProxyAuthorization)
            .is_none()
    );
    let body = conn.request_body_string().await.unwrap();
    let response = format!("{} {body}", conn.path_and_query());
    conn.ok(response)
}

/// Sends two requests through a proxy that both forwards and tunnels. An `http` origin is
/// forwarded and an `https` origin is tunneled; either way the policy's credentials reach the
/// proxy and never the origin.
async fn request_through_proxy(proxy_tls: bool, origin_tls: bool) -> TestResult {
    let proxy_cert = CertPair::generate(&["127.0.0.1"]);
    let origin_cert = CertPair::generate(&["localhost"]);

    let origin_config = trillium_smol::config().with_host("127.0.0.1").with_port(0);
    let origin = if origin_tls {
        origin_config
            .with_acceptor(origin_cert.acceptor())
            .spawn(origin)
    } else {
        origin_config.spawn(origin)
    };
    let origin_port = origin.info().await.tcp_socket_addr().unwrap().port();

    let proxy_handler = || {
        (
            |conn: Conn| async move {
                assert_eq!(
                    conn.request_headers()
                        .get_str(KnownHeaderName::ProxyAuthorization),
                    Some("Basic secret")
                );
                conn
            },
            ForwardProxyConnect::new(ClientConfig::default()),
            Proxy::new(Client::new(ClientConfig::default()), ForwardProxy),
        )
    };
    let proxy_config = trillium_smol::config().with_host("127.0.0.1").with_port(0);
    let proxy = if proxy_tls {
        proxy_config
            .with_acceptor(proxy_cert.acceptor())
            .spawn(proxy_handler())
    } else {
        proxy_config.spawn(proxy_handler())
    };
    let proxy_addr = *proxy.info().await.tcp_socket_addr().unwrap();

    let proxy_scheme = if proxy_tls { "https" } else { "http" };
    let proxy_route = ClientProxy::new(format!("{proxy_scheme}://{proxy_addr}"))?
        .with_connect_header(KnownHeaderName::ProxyAuthorization, "Basic secret");
    let client = tls_client(&[&proxy_cert, &origin_cert]).with_proxy(proxy_route);
    let origin_url = if origin_tls {
        format!("https://localhost:{origin_port}/path?q=1")
    } else {
        format!("http://127.0.0.1:{origin_port}/path?q=1")
    };
    for _ in 0..2 {
        let mut conn = client
            .post(origin_url.as_str())
            .with_body("payload")
            .await?;
        assert_eq!(conn.status().unwrap(), 200);
        assert_eq!(
            conn.response_body().read_string().await?,
            "/path?q=1 payload"
        );
        conn.take_response_body().unwrap().recycle().await;
    }
    drop(client);
    proxy.shut_down().await;
    origin.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn forwards_http_through_plaintext_proxy() -> TestResult {
    request_through_proxy(false, false).await
}

#[test(harness)]
async fn forwards_http_through_tls_proxy() -> TestResult {
    request_through_proxy(true, false).await
}

#[test(harness)]
async fn tunnels_https_through_plaintext_proxy() -> TestResult {
    request_through_proxy(false, true).await
}

#[test(harness)]
async fn tunnels_https_through_tls_proxy() -> TestResult {
    request_through_proxy(true, true).await
}

struct WithoutExtendedConnect;

impl trillium::Handler for WithoutExtendedConnect {
    async fn run(&self, conn: Conn) -> Conn {
        conn
    }

    async fn init(&mut self, info: &mut trillium::Info) {
        info.config_mut().set_extended_connect_enabled(false);
    }
}

async fn websocket_through_proxy(secure: bool) -> TestResult {
    use futures_lite::StreamExt;
    use std::sync::{Arc, Mutex};
    use trillium_client::{Version, WebSocketConn, websocket::Message};

    let cert = CertPair::generate(&["localhost"]);
    let versions = Arc::new(Mutex::new(Vec::new()));
    let observed = versions.clone();
    let handler = (
        move |conn: Conn| {
            observed.lock().unwrap().push(conn.http_version());
            async move {
                assert_eq!(conn.method(), trillium::Method::Get);
                assert_eq!(conn.request_headers().get_str("upgrade"), Some("websocket"));
                assert!(
                    conn.request_headers()
                        .get(KnownHeaderName::ProxyAuthorization)
                        .is_none()
                );
                conn
            }
        },
        trillium_websockets::websocket(|mut conn: WebSocketConn| async move {
            while let Some(Ok(Message::Text(text))) = conn.next().await {
                conn.send_string(format!("echo:{text}")).await.unwrap();
            }
        }),
        WithoutExtendedConnect,
    );
    let config = trillium_smol::config().with_host("127.0.0.1").with_port(0);
    let origin = if secure {
        config.with_acceptor(cert.acceptor()).spawn(handler)
    } else {
        config.spawn(handler)
    };
    let origin_port = origin.info().await.tcp_socket_addr().unwrap().port();
    let hops = Arc::new(Mutex::new(Vec::new()));
    let observed = hops.clone();
    let proxy = trillium_smol::config()
        .with_host("127.0.0.1")
        .with_port(0)
        .spawn((
            move |conn: Conn| {
                assert_eq!(conn.method(), trillium::Method::Connect);
                assert!(conn.request_headers().get("upgrade").is_none());
                let inner: &trillium_http::Conn<_> = conn.as_ref();
                observed
                    .lock()
                    .unwrap()
                    .push(inner.authority().unwrap().to_owned());
                async move { conn }
            },
            ForwardProxyConnect::new(ClientConfig::default()),
        ));
    let proxy_addr = *proxy.info().await.tcp_socket_addr().unwrap();
    let client = tls_client(&[&cert]).with_proxy(ClientProxy::new(format!("http://{proxy_addr}"))?);
    let scheme = if secure { "https" } else { "http" };
    let mut ws = client
        .get(format!("{scheme}://localhost:{origin_port}/socket"))
        .into_websocket()
        .await?;
    ws.send_string("hello".into()).await?;
    assert_eq!(ws.next().await.unwrap()?, Message::text("echo:hello"));
    drop(ws);
    // The h2 connection fails its extended-CONNECT gate before sending a request. Its retry
    // must open a second tunnel, not connect straight to the origin or reuse forward mode.
    let expected_hops = if secure { 2 } else { 1 };
    assert_eq!(
        *hops.lock().unwrap(),
        vec![format!("localhost:{origin_port}"); expected_hops]
    );
    assert_eq!(*versions.lock().unwrap(), [Version::Http1_1]);
    drop(client);
    proxy.shut_down().await;
    origin.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn ws_uses_connect_through_forward_proxy() -> TestResult {
    websocket_through_proxy(false).await
}

#[test(harness)]
async fn wss_h2_fallback_keeps_tunnel_route() -> TestResult {
    websocket_through_proxy(true).await
}
