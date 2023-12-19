use std::net::IpAddr;
use trillium_forwarding::*;
use trillium_testing::prelude::*;

fn app(forwarding: Forwarding) -> impl trillium::Handler {
    (forwarding, |conn: Conn| async move {
        let response = format!(
            "{:?} {:?} {:?}",
            conn.inner().is_secure(),
            conn.inner().peer_ip(),
            conn.inner().host()
        );
        conn.ok(response)
    })
}

#[test]
fn test_always() {
    let app = app(Forwarding::trust_always());

    assert_ok!(
        get("/")
            .with_request_header("forwarded", "for=192.0.2.60;proto=https;by=203.0.113.43")
            .with_peer_ip("203.0.113.43".parse().unwrap())
            .on(&app),
        "true Some(192.0.2.60) None"
    );

    assert_ok!(
        get("/")
            .with_request_header("forwarded", "for=192.0.2.60;proto=https;by=203.0.113.43")
            .on(&app),
        "true Some(192.0.2.60) None"
    );

    assert_ok!(
        get("/")
            .with_request_header("x-forwarded-for", "192.0.2.60")
            .with_request_header("x-forwarded-proto", "https")
            .with_request_header("x-forwarded-host", "example.com")
            .with_peer_ip("203.0.113.43".parse().unwrap())
            .on(&app),
        "true Some(192.0.2.60) Some(\"example.com\")"
    );

    assert_ok!(get("/").on(&app), "false None None");
}

#[test]
fn test_loopback() {
    let app = app(Forwarding::trust_fn(IpAddr::is_loopback));

    assert_ok!(
        get("/")
            .with_request_header(
                "forwarded",
                "for=192.0.2.60;proto=https;host=example.com;by=127.0.0.1"
            )
            .with_peer_ip("127.0.0.1".parse().unwrap())
            .on(&app),
        "true Some(192.0.2.60) Some(\"example.com\")"
    );

    assert_ok!(
        get("/")
            .with_request_header("forwarded", "for=192.0.2.60;proto=https")
            .with_peer_ip("::1".parse().unwrap())
            .on(&app),
        "true Some(192.0.2.60) None"
    );

    assert_ok!(
        get("/")
            .with_request_header("forwarded", "for=192.0.2.60;proto=https")
            .with_peer_ip("10.1.10.1".parse().unwrap())
            .on(&app),
        "false Some(10.1.10.1) None"
    );
}

#[test]
fn test_ipranges() {
    let app = app(Forwarding::trust_ips(["10.10.10.10", "192.168.0.0/16"]));

    assert_ok!(
        get("/")
            .with_request_header("forwarded", "for=192.0.2.60;proto=https;host=example.com")
            .with_peer_ip("10.10.10.10".parse().unwrap())
            .on(&app),
        "true Some(192.0.2.60) Some(\"example.com\")"
    );

    assert_ok!(
        get("/")
            .with_request_header("forwarded", "for=192.0.2.60;proto=https;host=example.com")
            .with_peer_ip("192.168.1.1".parse().unwrap())
            .on(&app),
        "true Some(192.0.2.60) Some(\"example.com\")"
    );

    assert_ok!(
        get("/")
            .with_request_header("forwarded", "for=192.0.2.60;proto=https")
            .with_peer_ip("10.10.10.1".parse().unwrap())
            .on(&app),
        "false Some(10.10.10.1) None"
    );

    assert_ok!(
        get("/")
            .with_request_header("forwarded", "for=192.0.2.60;proto=https")
            .with_peer_ip("192.169.1.1".parse().unwrap())
            .on(&app),
        "false Some(192.169.1.1) None"
    );
}
