use futures_lite::{AsyncRead, AsyncWrite, io::AsyncReadExt};
use std::{
    borrow::Cow,
    collections::VecDeque,
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use trillium_client::{
    Client, Error, Headers, KnownHeaderName, Proxy, ProxyConnectError, Status, Version,
};
use trillium_server_common::{Connector, Destination, Runtime, Transport, Url};
use trillium_testing::{TestResult, harness, test};

#[path = "proxy_routing/h2.rs"]
mod h2;
#[path = "proxy_routing/recording.rs"]
mod recording;
use h2::H2Connector;
use recording::{fixture, proxy};

#[test(harness)]
async fn forward_target_and_credentials() -> TestResult {
    let (client, records) = fixture();
    let client = client.with_proxy(proxy("Basic proxy-secret"));
    let mut conn = client
        .get("http://user:pass@origin.example:81/a?q=1#fragment")
        .with_request_header(KnownHeaderName::ProxyAuthorization, "untrusted")
        .await?;
    assert_eq!(conn.response_body().read_string().await?, "ok");
    let records = records.lock().unwrap();
    assert_eq!(records.hops, ["proxy.example"]);
    assert!(records.resolutions.is_empty());
    let request = &records.requests[0];
    assert!(request.starts_with("GET http://origin.example:81/a?q=1 HTTP/1.1\r\n"));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("host: origin.example:81\r\n"),
        "{request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("proxy-authorization: basic proxy-secret\r\n"),
        "{request}"
    );
    assert!(!request.contains("untrusted"));
    assert!(!request.contains("x-connect-only"));
    Ok(())
}

#[test(harness)]
async fn tunnel_and_direct_strip_proxy_credentials() -> TestResult {
    let (client, records) = fixture();
    let proxied = client.clone().with_proxy(proxy("secret"));
    let bypassed = client.with_proxy(|_: &Url| None::<Proxy>);
    for client in [&bypassed, &proxied] {
        let mut conn = client
            .get("https://origin.example/a")
            .with_request_header(KnownHeaderName::ProxyAuthorization, "must-not-leak")
            .await?;
        assert_eq!(conn.response_body().read_string().await?, "ok");
    }
    let records = records.lock().unwrap();
    assert_eq!(records.hops, ["origin.example", "proxy.example"]);
    assert_eq!(records.tunnels, ["origin.example"]);
    assert_eq!(records.connect_requests.len(), 1);
    let connect_req = &records.connect_requests[0];
    assert!(connect_req.starts_with("CONNECT origin.example:443 HTTP/1.1\r\n"));
    assert!(
        connect_req
            .to_ascii_lowercase()
            .contains("proxy-authorization: secret\r\n")
    );
    assert!(
        connect_req
            .to_ascii_lowercase()
            .contains("x-connect-only: private\r\n")
    );
    assert!(!connect_req.contains("must-not-leak"));
    assert!(
        records
            .requests
            .iter()
            .all(|request| request.starts_with("GET /a HTTP/1.1\r\n")
                && !request.to_ascii_lowercase().contains("proxy-authorization")
                && !request.contains("x-connect-only"))
    );
    Ok(())
}

#[test(harness)]
async fn https_proxy_endpoint_is_dialed_secure_with_h1_alpn() -> TestResult {
    let (client, records) = fixture();
    let client = client.with_proxy(Proxy::new("https://proxy.example:8443")?);
    let mut http_conn = client.get("http://origin.example/plain").await?;
    assert_eq!(http_conn.response_body().read_string().await?, "ok");
    let mut https_conn = client.get("https://origin.example/tls").await?;
    assert_eq!(https_conn.response_body().read_string().await?, "ok");

    let records = records.lock().unwrap();
    assert_eq!(
        records.secure_hops,
        [
            ("proxy.example".to_owned(), true),
            ("proxy.example".to_owned(), true),
        ]
    );
    assert_eq!(records.tunnels, ["origin.example"]);
    Ok(())
}

#[test(harness)]
async fn detached_recycling_is_route_and_credentials_scoped() -> TestResult {
    let (client, records) = fixture();
    let first = client.clone().with_proxy(proxy("first"));
    let second = client.clone().with_proxy(proxy("second"));
    for client in [&client, &first, &first, &second, &second, &client] {
        let mut conn = client.get("http://origin.example/a").await?;
        conn.take_response_body().unwrap().recycle().await;
    }
    assert_eq!(
        records.lock().unwrap().hops,
        ["origin.example", "proxy.example", "proxy.example"]
    );
    Ok(())
}

#[test(harness)]
async fn h3_hint_uses_proxy_without_direct_fallback() -> TestResult {
    let (_, records) = fixture();
    let client = Client::new_with_quic(
        recording::RecordingConnector(records.clone()),
        trillium_quinn::ClientQuicConfig::with_webpki_roots(),
    )
    .with_proxy(proxy("secret"));
    for url in ["http://origin.example/", "https://origin.example/"] {
        let mut conn = client.get(url).with_http_version(Version::Http3).await?;
        assert_eq!(conn.http_version(), Version::Http1_1);
        assert_eq!(conn.response_body().read_string().await?, "ok");
    }
    let records = records.lock().unwrap();
    assert_eq!(records.hops, ["proxy.example", "proxy.example"]);
    assert_eq!(records.tunnels, ["origin.example"]);
    assert!(records.resolutions.is_empty());
    Ok(())
}

#[test(harness)]
async fn special_targets_are_preserved() -> TestResult {
    let (direct, records) = fixture();
    let mut conn = direct
        .build_conn("OPTIONS", "http://origin.example/")
        .with_request_target("*")
        .await?;
    conn.take_response_body().unwrap().recycle().await;

    let proxied = direct.with_proxy(proxy("secret"));
    let mut conn = proxied
        .build_conn("OPTIONS", "http://origin.example/")
        .with_request_target("*")
        .await?;
    conn.take_response_body().unwrap().recycle().await;
    let mut conn = proxied
        .build_conn("CONNECT", "http://origin.example:90/")
        .with_request_target("override.example:91")
        .await?;
    conn.take_response_body().unwrap().recycle().await;

    let records = records.lock().unwrap();
    assert!(records.requests[0].starts_with("OPTIONS * HTTP/1.1\r\n"));
    assert!(
        records.requests[1].starts_with("OPTIONS http://origin.example HTTP/1.1\r\n"),
        "{}",
        records.requests[1]
    );
    assert!(records.requests[2].starts_with("CONNECT override.example:91 HTTP/1.1\r\n"));
    assert_eq!(records.connect_requests.len(), 1);
    assert!(records.connect_requests[0].starts_with("CONNECT origin.example:90 HTTP/1.1\r\n"));
    assert_eq!(records.tunnels, ["origin.example"]);
    assert!(
        !records.requests[2]
            .to_ascii_lowercase()
            .contains("proxy-authorization")
    );
    Ok(())
}

#[test(harness)]
async fn without_policy_preserves_manual_proxy_authorization() -> TestResult {
    let (client, records) = fixture();
    let mut conn = client
        .get("http://origin.example/")
        .with_request_header(KnownHeaderName::ProxyAuthorization, "manual")
        .await?;
    conn.take_response_body().unwrap().recycle().await;
    assert!(
        records.lock().unwrap().requests[0]
            .to_ascii_lowercase()
            .contains("proxy-authorization: manual\r\n")
    );
    Ok(())
}

#[test(harness)]
async fn connect_refusal_reaches_the_caller_as_proxy_connect_error() {
    let (client, records) = fixture();
    records.lock().unwrap().connect_refusal = Some(
        b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"proxy\"\r\n\r\n"
            .to_vec(),
    );
    let client = client.with_proxy(proxy("secret"));
    let error = client
        .get("https://origin.example/secret")
        .await
        .expect_err("a refused tunnel must not produce a conn");
    let Error::Io(io_error) = error else {
        panic!("expected Error::Io, got {error:?}");
    };
    assert_eq!(io_error.kind(), io::ErrorKind::ConnectionRefused);
    let connect_error = io_error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<ProxyConnectError>())
        .expect("the CONNECT refusal should reach the caller intact");
    assert_eq!(connect_error.status(), Status::ProxyAuthenticationRequired);
    assert_eq!(
        connect_error.headers().get_str("proxy-authenticate"),
        Some("Basic realm=\"proxy\"")
    );

    let records = records.lock().unwrap();
    assert_eq!(records.hops, ["proxy.example"]);
    assert!(
        records.tunnels.is_empty(),
        "the origin must never be dialed after a refusal"
    );
    assert!(records.resolutions.is_empty());
}

#[test(harness)]
async fn proxy_failure_does_not_fall_back_to_direct() {
    let (client, records) = fixture();
    records.lock().unwrap().fail_connections = true;
    let client = client.with_proxy(proxy("secret"));
    assert!(client.get("http://origin.example/").await.is_err());
    assert!(client.get("https://origin.example/").await.is_err());
    let records = records.lock().unwrap();
    assert_eq!(records.hops, ["proxy.example", "proxy.example"]);
    assert!(records.tunnels.is_empty());
    assert!(records.resolutions.is_empty());
}

#[test(harness)]
async fn tunnel_h2_promotion_and_pooling_are_route_scoped() -> TestResult {
    let routes = Arc::new(Mutex::new(Vec::new()));
    let direct = Client::new(H2Connector {
        routes: routes.clone(),
    });
    let first = direct.clone().with_proxy(proxy("first"));
    let second = direct.clone().with_proxy(proxy("second"));
    for client in [&first, &first, &direct, &direct, &second, &second, &first] {
        let mut conn = client.get("https://origin.example/").await?;
        assert_eq!(conn.http_version(), Version::Http2);
        assert_eq!(conn.response_body().read_string().await?, "h2 body");
    }
    assert_eq!(*routes.lock().unwrap(), ["first", "direct", "second"]);
    Ok(())
}

#[derive(Debug)]
struct Followup;

impl trillium_client::ClientHandler for Followup {
    async fn after_response(
        &self,
        conn: &mut trillium_client::Conn,
    ) -> trillium_client::Result<()> {
        use trillium_client::ConnExt;
        if conn.url().host_str() == Some("first.example") {
            conn.set_followup(conn.client().get("http://second.example/followup"));
        }
        Ok(())
    }
}

#[test(harness)]
async fn followup_reselects_route_for_new_origin() -> TestResult {
    let (client, records) = fixture();
    let selections = Arc::new(Mutex::new(Vec::new()));
    let selected = selections.clone();
    let client = client.with_handler(Followup).with_proxy(move |url: &Url| {
        selected
            .lock()
            .unwrap()
            .push(url.host_str().unwrap().to_owned());
        (url.host_str() == Some("first.example")).then(|| proxy("secret"))
    });
    let mut conn = client.get("http://first.example/start").await?;
    assert_eq!(conn.url().host_str(), Some("second.example"));
    assert_eq!(conn.response_body().read_string().await?, "ok");
    assert_eq!(
        *selections.lock().unwrap(),
        ["first.example", "second.example"]
    );
    let records = records.lock().unwrap();
    assert_eq!(records.hops, ["proxy.example", "second.example"]);
    assert!(records.requests[0].contains("Proxy-Authorization: secret\r\n"));
    assert!(
        !records.requests[1]
            .to_ascii_lowercase()
            .contains("proxy-authorization")
    );
    Ok(())
}

#[test(harness)]
async fn h2c_hint_through_proxy_uses_tunnel() -> TestResult {
    let routes = Arc::new(Mutex::new(Vec::new()));
    let client = Client::new(H2Connector {
        routes: routes.clone(),
    })
    .with_proxy(proxy("h2c"));
    for _ in 0..2 {
        let mut conn = client
            .get("http://origin.example/")
            .with_http_version(Version::Http2)
            .await?;
        assert_eq!(conn.http_version(), Version::Http2);
        assert_eq!(conn.response_body().read_string().await?, "h2 body");
    }
    assert_eq!(*routes.lock().unwrap(), ["h2c"]);
    Ok(())
}

#[test(harness)]
async fn h3_hint_through_proxy_can_negotiate_h2() -> TestResult {
    let routes = Arc::new(Mutex::new(Vec::new()));
    let client = Client::new(H2Connector {
        routes: routes.clone(),
    })
    .with_proxy(proxy("h3-hint"));
    for _ in 0..2 {
        let mut conn = client
            .get("https://origin.example/")
            .with_http_version(Version::Http3)
            .await?;
        assert_eq!(conn.http_version(), Version::Http2);
        assert_eq!(conn.response_body().read_string().await?, "h2 body");
    }
    assert_eq!(*routes.lock().unwrap(), ["h3-hint"]);
    Ok(())
}
