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

#[test(harness)]
async fn multiple_forwarded_for_entries() {
    let app = TestServer::new(app(Forwarding::trust_ips([
        "10.10.10.10",
        "192.168.0.0/16",
    ])))
    .await
    .with_host("original");

    // the client-supplied prefix of the chain is ignored; the rightmost entry that isn't a
    // trusted proxy wins
    app.get("/")
        .with_request_header("x-forwarded-for", "1.2.3.4, 203.0.113.9")
        .with_peer_ip([10, 10, 10, 10])
        .await
        .assert_body(r#"false Some(203.0.113.9) Some("original")"#);

    // trusted proxies in the chain are walked through
    app.get("/")
        .with_request_header("x-forwarded-for", "1.2.3.4, 203.0.113.9, 192.168.1.1")
        .with_peer_ip([10, 10, 10, 10])
        .await
        .assert_body(r#"false Some(203.0.113.9) Some("original")"#);

    // ports are discarded
    app.get("/")
        .with_request_header(
            "forwarded",
            r#"for="1.2.3.4:1111", for="[2001:db8:cafe::17]:4711""#,
        )
        .with_peer_ip([10, 10, 10, 10])
        .await
        .assert_body(r#"false Some(2001:db8:cafe::17) Some("original")"#);

    // an entry that isn't an ip address ends the walk
    app.get("/")
        .with_request_header(
            "forwarded",
            r#"for=1.2.3.4, for="_gazonk", for=192.168.1.1"#,
        )
        .with_peer_ip([10, 10, 10, 10])
        .await
        .assert_body(r#"false Some(192.168.1.1) Some("original")"#);

    // every entry is trusted, so the walk reaches the leftmost
    app.get("/")
        .with_request_header("x-forwarded-for", "1.2.3.4, 192.168.1.1, 192.168.1.2")
        .with_peer_ip([10, 10, 10, 10])
        .await
        .assert_body(r#"false Some(1.2.3.4) Some("original")"#);
}
