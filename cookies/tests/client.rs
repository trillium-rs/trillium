//! Tests for the client-side `Cookies` handler.
//!
//! These tests use [`ServerConnector`] with a fixed server-side handler to drive round-trips and
//! verify that the cookie jar correctly captures `Set-Cookie` headers, attaches `Cookie` headers
//! on subsequent matching requests, and respects domain scoping.

use trillium::Conn as ServerConn;
use trillium_client::{Client, KnownHeaderName::Cookie, Status};
use trillium_cookies::client::Cookies;
use trillium_testing::{ServerConnector, TestResult, harness, test};

/// Server handler that emits a session-scoped `Set-Cookie` for every request.
async fn sets_session_cookie(conn: ServerConn) -> ServerConn {
    conn.with_response_header("set-cookie", "session=abc; Path=/")
        .ok("ok")
}

#[test(harness)]
async fn first_request_has_no_cookie_header_subsequent_request_does() -> TestResult {
    let client =
        Client::new(ServerConnector::new(sets_session_cookie)).with_handler(Cookies::new());

    // First request: jar is empty, no Cookie header attached.
    let conn1 = client.get("http://example.com/").await?;
    assert_eq!(conn1.status(), Some(Status::Ok));
    assert!(
        conn1.request_headers().get_str(Cookie).is_none(),
        "expected no Cookie on first request, got {:?}",
        conn1.request_headers().get_str(Cookie),
    );

    // Second request: jar has session=abc from response 1, attached as Cookie.
    let conn2 = client.get("http://example.com/").await?;
    assert_eq!(conn2.request_headers().get_str(Cookie), Some("session=abc"),);
    Ok(())
}

#[test(harness)]
async fn cookies_do_not_cross_domains() -> TestResult {
    let client =
        Client::new(ServerConnector::new(sets_session_cookie)).with_handler(Cookies::new());

    // Set a cookie on example.com.
    let _ = client.get("http://example.com/").await?;

    // Request to other.com should NOT include the example.com cookie.
    let conn = client.get("http://other.com/").await?;
    assert!(
        conn.request_headers().get_str(Cookie).is_none(),
        "cookie leaked across domains: {:?}",
        conn.request_headers().get_str(Cookie),
    );
    Ok(())
}
