use std::net::IpAddr;
use trillium::{Conn, KnownHeaderName};
use trillium_forwarding::*;
use trillium_testing::{TestServer, harness, test};

fn app(forwarding: Forwarding) -> impl trillium::Handler {
    (forwarding, |conn: Conn| async move {
        let response = format!(
            "{:?} {:?} {:?}",
            conn.is_secure(),
            conn.peer_ip(),
            conn.host()
        );
        conn.ok(response)
    })
}

#[test(harness)]
async fn test_always() {
    let app = TestServer::new(app(Forwarding::trust_always()))
        .await
        .with_host("original");

    app.get("/")
        .with_request_header("forwarded", "for=192.0.2.60;proto=https;by=203.0.113.43")
        .with_peer_ip([203, 0, 113, 43])
        .with_request_header(KnownHeaderName::Host, "original")
        .await
        .assert_ok()
        .assert_body(r#"true Some(192.0.2.60) Some("original")"#);

    app.get("/")
        .with_request_header("forwarded", "for=192.0.2.60;proto=https;by=203.0.113.43")
        .with_request_header(KnownHeaderName::Host, "original")
        .await
        .assert_ok()
        .assert_body(r#"true Some(192.0.2.60) Some("original")"#);

    app.get("/")
        .with_request_header("x-forwarded-for", "192.0.2.60")
        .with_request_header("x-forwarded-proto", "https")
        .with_request_header("x-forwarded-host", "example.com")
        .with_peer_ip([203, 0, 113, 43])
        .await
        .assert_body(r#"true Some(192.0.2.60) Some("example.com")"#);

    app.get("/")
        .await
        .assert_body(r#"false None Some("original")"#);
}

#[test(harness)]
async fn test_loopback() {
    let app = TestServer::new(app(Forwarding::trust_fn(IpAddr::is_loopback)))
        .await
        .with_host("original");

    app.get("/")
        .with_request_header(
            "forwarded",
            "for=192.0.2.60;proto=https;host=example.com;by=127.0.0.1",
        )
        .with_peer_ip([127, 0, 0, 1])
        .await
        .assert_body(r#"true Some(192.0.2.60) Some("example.com")"#);

    app.get("/")
        .with_request_header("forwarded", "for=192.0.2.60;proto=https")
        .with_request_header(KnownHeaderName::Host, "original")
        .with_peer_ip([0, 0, 0, 0, 0, 0, 0, 1])
        .await
        .assert_body(r#"true Some(192.0.2.60) Some("original")"#);

    app.get("/")
        .with_request_header("forwarded", "for=192.0.2.60;proto=https")
        .with_peer_ip([10, 1, 10, 1])
        .await
        .assert_body("false Some(10.1.10.1) Some(\"original\")");
}

#[test(harness)]
async fn test_ipranges() {
    let app = TestServer::new(app(Forwarding::trust_ips([
        "10.10.10.10",
        "192.168.0.0/16",
    ])))
    .await
    .with_host("original");

    app.get("/")
        .with_request_header("forwarded", "for=192.0.2.60;proto=https;host=example.com")
        .with_peer_ip([10, 10, 10, 10])
        .await
        .assert_body("true Some(192.0.2.60) Some(\"example.com\")");

    app.get("/")
        .with_request_header("forwarded", "for=192.0.2.60;proto=https;host=example.com")
        .with_peer_ip([192, 168, 1, 1])
        .await
        .assert_body("true Some(192.0.2.60) Some(\"example.com\")");

    app.get("/")
        .with_request_header("forwarded", "for=192.0.2.60;proto=https")
        .with_peer_ip([10, 10, 10, 1])
        .await
        .assert_body("false Some(10.10.10.1) Some(\"original\")");

    app.get("/")
        .with_request_header("forwarded", "for=192.0.2.60;proto=https")
        .with_peer_ip([192, 169, 1, 1])
        .await
        .assert_body("false Some(192.169.1.1) Some(\"original\")");
}
