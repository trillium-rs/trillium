use super::*;
use crate::{Client, Proxy};
use trillium_http::Method;

const HTTP: &str = "http://origin.example/";
const HTTPS: &str = "https://origin.example/";

fn route(client: &Client, url: &str, setup: impl FnOnce(&mut Conn)) -> &'static str {
    let mut conn = client.build_conn(Method::Get, url);
    setup(&mut conn);
    conn.select_route();
    match conn.route {
        Route::Direct => "direct",
        Route::Forward(_) => "forward",
        Route::Tunnel(_) => "tunnel",
    }
}

#[test]
fn route_selection() {
    let direct = Client::new(trillium_smol::ClientConfig::default());
    let proxied = direct
        .clone()
        .with_proxy(Proxy::new("http://proxy.example:8080").unwrap());

    assert_eq!(route(&direct, HTTP, |_| {}), "direct");
    assert_eq!(route(&proxied, HTTP, |_| {}), "forward");
    assert_eq!(route(&proxied, HTTPS, |_| {}), "tunnel");
    assert_eq!(
        route(&proxied, HTTP, |c| c.method = Method::Connect),
        "tunnel"
    );
    assert_eq!(
        route(&proxied, HTTP, |c| c.protocol = Some("websocket".into())),
        "tunnel"
    );
    assert_eq!(route(&proxied, HTTP, |c| c.upgrade = true), "tunnel");
    let upgrade_header = |c: &mut Conn| {
        c.request_headers.insert(KnownHeaderName::Upgrade, "custom");
    };
    assert_eq!(route(&proxied, HTTP, upgrade_header), "tunnel");
    assert_eq!(
        route(&proxied, HTTP, |c| c.http_version = Some(Version::Http2)),
        "tunnel"
    );
    // An h3 hint continues over h1 through the proxy, so it needs no tunnel of its own.
    assert_eq!(
        route(&proxied, HTTP, |c| c.http_version = Some(Version::Http3)),
        "forward"
    );
}
