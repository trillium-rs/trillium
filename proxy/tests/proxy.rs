use trillium::{Conn, Handler, KnownHeaderName, Status};
use trillium_proxy::{Client, Proxy, Url, upstream::RoundRobin};
use trillium_testing::{ServerConnector, TestResult, TestServer, harness, test};

const UPSTREAM: &str = "http://upstream.example";

/// An upstream handler that reflects everything the proxy forwarded back to the
/// caller: the received method, request target, and a handful of headers become
/// `echoed-*` response headers, and the request body is echoed into the response
/// body. This lets a `TestServer` in front of the proxy assert on exactly what
/// reached the upstream.
async fn echo(mut conn: Conn) -> Conn {
    let body = conn.request_body_string().await.unwrap_or_default();
    let method = conn.method().to_string();
    let target = conn.path_and_query().to_string();

    let headers = conn.request_headers();
    let get = |name: KnownHeaderName| headers.get_str(name).unwrap_or("(absent)").to_string();
    let forwarded = get(KnownHeaderName::Forwarded);
    let via = get(KnownHeaderName::Via);
    let host = get(KnownHeaderName::Host);
    let x_forwarded_for = get(KnownHeaderName::XforwardedFor);
    let content_length = get(KnownHeaderName::ContentLength);
    let custom = headers
        .get_str("x-custom")
        .unwrap_or("(absent)")
        .to_string();
    let header_names = headers
        .iter()
        .map(|(name, _)| name.to_string().to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(",");

    conn.with_response_header("echoed-method", method)
        .with_response_header("echoed-target", target)
        .with_response_header("echoed-forwarded", forwarded)
        .with_response_header("echoed-via", via)
        .with_response_header("echoed-host", host)
        .with_response_header("echoed-x-forwarded-for", x_forwarded_for)
        .with_response_header("echoed-content-length", content_length)
        .with_response_header("echoed-custom", custom)
        .with_response_header("echoed-request-header-names", header_names)
        .with_response_header(KnownHeaderName::Server, "upstream-server")
        .with_body(format!("upstream received: {body}"))
        .with_status(Status::Ok)
}

async fn not_found(conn: Conn) -> Conn {
    conn.with_status(Status::NotFound)
}

async fn add_marker(conn: Conn) -> Conn {
    conn.with_response_header("x-fallthrough", "reached")
}

/// Stands in for the `Server` header a real trillium stack sets on every response
/// before the handler runs, so the proxy tests can exercise its removal.
async fn set_own_server_header(conn: Conn) -> Conn {
    conn.with_response_header(KnownHeaderName::Server, "trillium-proxy-itself")
}

async fn no_server_upstream(conn: Conn) -> Conn {
    conn.with_status(Status::Ok).with_body("ok")
}

async fn status_from_header(conn: Conn) -> Conn {
    let status: u16 = conn
        .request_headers()
        .get_str("x-status")
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);
    conn.with_status(status)
}

async fn two_cookies(mut conn: Conn) -> Conn {
    conn.response_headers_mut()
        .append(KnownHeaderName::SetCookie, "a=1");
    conn.response_headers_mut()
        .append(KnownHeaderName::SetCookie, "b=2");
    conn.with_status(Status::Ok)
}

async fn hop_by_hop_response(conn: Conn) -> Conn {
    conn.with_response_header(KnownHeaderName::KeepAlive, "timeout=5")
        .with_response_header(KnownHeaderName::ProxyAuthenticate, "Basic realm=upstream")
        .with_response_header("x-legit", "kept")
        .with_status(Status::Ok)
}

fn client_for(upstream: impl Handler) -> Client {
    Client::new(ServerConnector::new(upstream))
}

async fn proxy_server(upstream: impl Handler) -> TestServer<Proxy<Url>> {
    TestServer::new(Proxy::new(client_for(upstream), UPSTREAM)).await
}

#[test(harness)]
async fn proxies_status_body_and_upstream_response_headers() -> TestResult {
    let server = proxy_server(echo).await;
    server
        .get("/hello")
        .await
        .assert_status(200)
        .assert_body("upstream received: ")
        .assert_header("echoed-method", "GET");
    Ok(())
}

#[test(harness)]
async fn request_target_is_forwarded_including_query_and_colons() -> TestResult {
    let server = proxy_server(echo).await;
    // The colon in the first path segment is a regression guard: `Url::join`
    // would otherwise read `trillium:` as a scheme and produce a non-base url.
    server
        .get("/trillium::Handler?x=1&y=2")
        .await
        .assert_status(200)
        .assert_header("echoed-target", "/trillium::Handler?x=1&y=2");
    Ok(())
}

#[test(harness)]
async fn crafted_path_cannot_redirect_to_a_foreign_host() -> TestResult {
    // A path embedding an absolute url must be joined as a path onto the
    // configured upstream, not honored as a new host. `echoed-host` is the
    // authority the proxy's client actually dialed.
    let server = proxy_server(echo).await;
    server
        .get("/https://sneaky.example")
        .await
        .assert_header("echoed-host", "upstream.example")
        .assert_header("echoed-target", "/https://sneaky.example");
    Ok(())
}

#[test(harness)]
async fn upstream_base_path_is_joined_with_request_target() -> TestResult {
    let server = TestServer::new(Proxy::new(
        client_for(echo),
        "http://upstream.example/prefix/",
    ))
    .await;
    server
        .get("/thing?q=1")
        .await
        .assert_header("echoed-target", "/prefix/thing?q=1");
    Ok(())
}

#[test(harness)]
async fn request_body_is_streamed_to_upstream() -> TestResult {
    let server = proxy_server(echo).await;
    server
        .post("/")
        .with_body("hello upstream")
        .await
        .assert_status(200)
        .assert_body("upstream received: hello upstream");
    Ok(())
}

#[test(harness)]
async fn forwards_custom_headers_and_strips_x_forwarded_for() -> TestResult {
    let server = proxy_server(echo).await;
    server
        .get("/")
        .with_request_header("x-custom", "kept")
        .with_request_header(KnownHeaderName::XforwardedFor, "9.9.9.9")
        .await
        .assert_header("echoed-custom", "kept")
        // hop-by-hop / forwarding headers are rewritten into `Forwarded`, so the
        // client's `x-forwarded-for` must not reach the upstream verbatim.
        .assert_header("echoed-x-forwarded-for", "(absent)");
    Ok(())
}

#[test(harness)]
async fn adds_forwarded_header_with_host_and_peer_ip() -> TestResult {
    let server = proxy_server(echo).await;
    let conn = server.get("/").with_peer_ip([10, 0, 0, 1]).await;
    let forwarded = conn.header("echoed-forwarded").unwrap();
    assert!(forwarded.contains("trillium.test"), "{forwarded}");
    assert!(forwarded.contains("10.0.0.1"), "{forwarded}");
    Ok(())
}

#[test(harness)]
async fn via_pseudonym_is_added_to_request_and_response() -> TestResult {
    let server =
        TestServer::new(Proxy::new(client_for(echo), UPSTREAM).with_via_pseudonym("test-proxy"))
            .await;
    let conn = server
        .get("/")
        .with_request_header(KnownHeaderName::Via, "1.0 old-proxy")
        .await;

    let echoed_via = conn.header("echoed-via").unwrap();
    assert!(echoed_via.contains("test-proxy"), "{echoed_via}");
    assert!(echoed_via.contains("old-proxy"), "{echoed_via}");

    let via = conn.header(KnownHeaderName::Via).unwrap();
    assert!(via.contains("test-proxy"), "{via}");
    Ok(())
}

#[test(harness)]
async fn upstream_server_header_replaces_the_proxys_own() -> TestResult {
    // Transparent passthrough (matching nginx's opposite aside — caddy and traefik
    // both do this): the proxy strips whatever `Server` its own stack set and
    // presents the upstream's, never advertising two.
    let server = TestServer::new((
        set_own_server_header,
        Proxy::new(client_for(echo), UPSTREAM),
    ))
    .await;
    server
        .get("/")
        .await
        .assert_header(KnownHeaderName::Server, "upstream-server");
    Ok(())
}

#[test(harness)]
async fn server_header_is_stripped_when_upstream_sends_none() -> TestResult {
    // The other direction of transparency: if the upstream sends no `Server`, the
    // proxy forwards none either, rather than leaking its own identity.
    let server = TestServer::new((
        set_own_server_header,
        Proxy::new(client_for(no_server_upstream), UPSTREAM),
    ))
    .await;
    server.get("/").await.assert_no_header("server");
    Ok(())
}

#[test(harness)]
async fn not_found_passes_through_by_default() -> TestResult {
    let server = TestServer::new((Proxy::new(client_for(not_found), UPSTREAM), add_marker)).await;
    // Default behavior: a 404 from upstream is not forwarded; the conn passes
    // unhalted to the next handler in the tuple.
    server
        .get("/")
        .await
        .assert_header("x-fallthrough", "reached");
    Ok(())
}

#[test(harness)]
async fn proxy_not_found_forwards_the_404_and_halts() -> TestResult {
    let server = TestServer::new((
        Proxy::new(client_for(not_found), UPSTREAM).proxy_not_found(),
        add_marker,
    ))
    .await;
    server
        .get("/")
        .await
        .assert_status(404)
        .assert_no_header("x-fallthrough");
    Ok(())
}

#[test(harness)]
async fn halts_by_default() -> TestResult {
    let server = TestServer::new((Proxy::new(client_for(echo), UPSTREAM), add_marker)).await;
    server
        .get("/")
        .await
        .assert_status(200)
        .assert_no_header("x-fallthrough");
    Ok(())
}

#[test(harness)]
async fn without_halting_continues_to_next_handler() -> TestResult {
    let server = TestServer::new((
        Proxy::new(client_for(echo), UPSTREAM).without_halting(),
        add_marker,
    ))
    .await;
    server
        .get("/")
        .await
        .assert_status(200)
        .assert_body_contains("upstream received")
        .assert_header("x-fallthrough", "reached");
    Ok(())
}

#[test(harness)]
async fn round_robin_alternates_between_upstreams() -> TestResult {
    let selector = RoundRobin::new(["http://a.example", "http://b.example"]);
    let server = TestServer::new(Proxy::new(client_for(echo), selector)).await;

    let mut hosts = Vec::new();
    for _ in 0..4 {
        let conn = server.get("/").await;
        hosts.push(conn.header("echoed-host").unwrap().to_string());
    }

    assert_eq!(hosts, ["a.example", "b.example", "a.example", "b.example"]);
    Ok(())
}

#[test(harness)]
async fn request_hop_by_hop_headers_are_not_forwarded() -> TestResult {
    let server = proxy_server(echo).await;
    let names = server
        .get("/")
        .with_request_header(KnownHeaderName::ProxyAuthorization, "secret")
        .with_request_header(KnownHeaderName::KeepAlive, "timeout=5")
        .with_request_header(KnownHeaderName::Te, "trailers")
        .with_request_header(KnownHeaderName::Upgrade, "h2c")
        .with_request_header("x-legit", "v")
        .await
        .header("echoed-request-header-names")
        .unwrap()
        .to_string();

    let names: Vec<&str> = names.split(',').collect();
    for hop in ["proxy-authorization", "keep-alive", "te", "upgrade"] {
        assert!(!names.contains(&hop), "{hop:?} leaked upstream: {names:?}");
    }
    assert!(
        names.contains(&"x-legit"),
        "end-to-end header dropped: {names:?}"
    );
    Ok(())
}

#[test(harness)]
async fn response_hop_by_hop_headers_are_not_forwarded() -> TestResult {
    let server = proxy_server(hop_by_hop_response).await;
    server
        .get("/")
        .await
        .assert_no_header("keep-alive")
        .assert_no_header("proxy-authenticate")
        .assert_header("x-legit", "kept");
    Ok(())
}

#[test(harness)]
async fn multiple_set_cookie_headers_are_preserved() -> TestResult {
    let server = proxy_server(two_cookies).await;
    let conn = server.get("/").await;
    let cookies = conn
        .response_headers()
        .get_values(KnownHeaderName::SetCookie)
        .expect("set-cookie present");
    let values: Vec<String> = cookies.iter().map(ToString::to_string).collect();
    assert_eq!(values.len(), 2, "{values:?}");
    assert!(values.iter().any(|v| v == "a=1"), "{values:?}");
    assert!(values.iter().any(|v| v == "b=2"), "{values:?}");
    Ok(())
}

#[test(harness)]
async fn request_method_is_preserved() -> TestResult {
    let server = proxy_server(echo).await;
    for method in ["PUT", "DELETE", "PATCH", "OPTIONS", "TRACE", "HEAD"] {
        server
            .build(method, "/")
            .await
            .assert_header("echoed-method", method);
    }
    Ok(())
}

#[test(harness)]
async fn head_response_body_is_suppressed() -> TestResult {
    // RFC 9110 §9.3.2: a HEAD response carries the headers a GET would, but no
    // body. The upstream `echo` always writes a body; the proxy must not relay it.
    let server = proxy_server(echo).await;
    let conn = server.build("HEAD", "/thing").await;
    conn.assert_status(200)
        .assert_header("echoed-method", "HEAD");
    assert_eq!(conn.body(), "", "HEAD response leaked a body");
    Ok(())
}

#[test(harness)]
async fn response_status_is_forwarded_verbatim() -> TestResult {
    let server = proxy_server(status_from_header).await;
    for code in [201u16, 301, 418, 500, 204] {
        server
            .get("/")
            .with_request_header("x-status", code.to_string())
            .await
            .assert_status(code);
    }
    Ok(())
}

#[test(harness)]
async fn large_bodies_stream_intact_in_both_directions() -> TestResult {
    let server = proxy_server(echo).await;
    let big = "x".repeat(200_000);
    let conn = server.post("/").with_body(big.clone()).await;
    conn.assert_status(200);
    let body = conn.body();
    let payload = body
        .strip_prefix("upstream received: ")
        .expect("body prefix present");
    assert_eq!(payload.len(), big.len());
    assert!(payload.bytes().all(|b| b == b'x'));
    Ok(())
}

#[test(harness)]
async fn incoming_forwarded_header_is_extended_not_replaced() -> TestResult {
    let server = proxy_server(echo).await;
    let conn = server
        .get("/")
        .with_request_header(KnownHeaderName::Forwarded, "for=1.1.1.1")
        .with_peer_ip([2, 2, 2, 2])
        .await;
    let forwarded = conn.header("echoed-forwarded").unwrap();
    assert!(
        forwarded.contains("1.1.1.1"),
        "incoming dropped: {forwarded}"
    );
    assert!(forwarded.contains("2.2.2.2"), "peer not added: {forwarded}");
    assert!(
        forwarded.contains("trillium.test"),
        "host not added: {forwarded}"
    );
    Ok(())
}

#[test(harness)]
async fn percent_encoded_path_is_forwarded_without_reencoding() -> TestResult {
    let server = proxy_server(echo).await;
    // A reverse proxy should forward the request-target opaquely; encoded octets
    // like `%2F` (a literal slash) must not be decoded, or the upstream sees a
    // different path than the client sent.
    server
        .get("/a%2Fb%20c")
        .await
        .assert_header("echoed-target", "/a%2Fb%20c");
    Ok(())
}

#[test(harness)]
async fn root_and_query_only_targets_are_forwarded() -> TestResult {
    let server = proxy_server(echo).await;
    server.get("/").await.assert_header("echoed-target", "/");
    server
        .get("/?q=1&r=2")
        .await
        .assert_header("echoed-target", "/?q=1&r=2");
    Ok(())
}

#[test(harness)]
async fn dot_and_empty_segments_are_normalized() -> TestResult {
    // trillium resolves `.`/`..` and collapses empty segments when composing the
    // upstream target (it does not relay them verbatim), and refuses targets that
    // escape the base path — the escape-containment guarantee is unit-tested at
    // the selector level in `upstream.rs`. This pins the observable normalization.
    let server = proxy_server(echo).await;
    server
        .get("/a/../b")
        .await
        .assert_header("echoed-target", "/b");
    server
        .get("//a//b")
        .await
        .assert_header("echoed-target", "/a//b");
    Ok(())
}

#[test(harness)]
async fn connection_listed_header_is_stripped() -> TestResult {
    // RFC 9110 §7.6.1: a header named as a connection option in `Connection` must
    // be removed before forwarding. This is the dynamic counterpart to the static
    // hop-by-hop list exercised above.
    let server = proxy_server(echo).await;
    let names = server
        .get("/")
        .with_request_header(KnownHeaderName::Connection, "x-remove")
        .with_request_header("x-remove", "secret")
        .await
        .header("echoed-request-header-names")
        .unwrap()
        .to_string();
    assert!(
        !names.split(',').any(|n| n == "x-remove"),
        "connection-listed header leaked upstream: {names}"
    );
    Ok(())
}

#[test(harness)]
async fn response_content_length_matches_relayed_body() -> TestResult {
    async fn upstream(conn: Conn) -> Conn {
        conn.with_status(Status::Ok).with_body("hello world")
    }
    let server = proxy_server(upstream).await;
    let conn = server.get("/").await;
    conn.assert_body("hello world").assert_header(
        KnownHeaderName::ContentLength,
        conn.body().len().to_string().as_str(),
    );
    Ok(())
}

#[test(harness)]
async fn conflicting_request_content_length_is_not_relayed_verbatim() -> TestResult {
    // A `Content-Length` that under-counts the body is a smuggling setup. Whatever
    // the proxy forwards, the length the upstream sees must match the bytes it can
    // actually read — the attacker's under-count must not survive.
    let server = proxy_server(echo).await;
    let conn = server
        .post("/")
        .with_body("hello")
        .with_request_header(KnownHeaderName::ContentLength, "3")
        .await;
    let received = conn
        .body()
        .strip_prefix("upstream received: ")
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        conn.header("echoed-content-length"),
        Some(received.len().to_string().as_str()),
        "upstream content-length disagrees with the {} bytes it read",
        received.len()
    );
    Ok(())
}

#[test(harness)]
async fn expect_100_continue_request_delivers_body() -> TestResult {
    let server = proxy_server(echo).await;
    server
        .post("/")
        .with_request_header(KnownHeaderName::Expect, "100-continue")
        .with_body("payload")
        .await
        .assert_status(200)
        .assert_body("upstream received: payload");
    Ok(())
}

#[test(harness)]
async fn upstream_failure_yields_bad_gateway() -> TestResult {
    async fn boom(_conn: Conn) -> Conn {
        panic!("upstream exploded");
    }
    let server = proxy_server(boom).await;
    server.get("/").await.assert_status(502);
    Ok(())
}
