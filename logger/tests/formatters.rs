//! Per-formatter coverage for the server-side building blocks.

mod common;

use common::server;
use std::net::IpAddr;
use trillium::KnownHeaderName::{Host, Referer, UserAgent};
use trillium_logger::{
    ColorMode,
    formatters::{
        body_len_human, bytes, host, ip, method, referer, request_header, response_header,
        response_time, secure, status, url, user_agent, version,
    },
};
use trillium_testing::{harness, test};

#[test(harness)]
async fn method_renders() {
    let (app, target) = server(method, ColorMode::Off).await;
    app.get("/").await;
    assert_eq!(target.next().await, "GET");
}

#[test(harness)]
async fn version_renders() {
    let (app, target) = server(version, ColorMode::Off).await;
    app.get("/").await;
    assert_eq!(target.next().await, "HTTP/1.1");
}

#[test(harness)]
async fn url_includes_query() {
    let (app, target) = server(url, ColorMode::Off).await;
    app.get("/widgets?id=1&sort=asc").await;
    assert_eq!(target.next().await, "/widgets?id=1&sort=asc");
}

#[test(harness)]
async fn url_without_query() {
    let (app, target) = server(url, ColorMode::Off).await;
    app.get("/widgets").await;
    assert_eq!(target.next().await, "/widgets");
}

#[test(harness)]
async fn status_uncolored() {
    let (app, target) = server(status, ColorMode::Off).await;
    app.get("/").await;
    assert_eq!(target.next().await, "418");
}

#[test(harness)]
async fn status_colored_still_contains_code() {
    let (app, target) = server(status, ColorMode::On).await;
    app.get("/").await;
    // Whether or not ANSI codes survive the colored crate's own tty detection, the numeric code
    // must always be present.
    assert!(target.next().await.contains("418"));
}

#[test(harness)]
async fn ip_present() {
    let (app, target) = server(ip, ColorMode::Off).await;
    app.get("/").with_peer_ip(IpAddr::from([1, 2, 3, 4])).await;
    assert_eq!(target.next().await, "1.2.3.4");
}

#[test(harness)]
async fn ip_absent_renders_dash() {
    let (app, target) = server(ip, ColorMode::Off).await;
    app.get("/").await;
    assert_eq!(target.next().await, "-");
}

#[test(harness)]
async fn bytes_renders_raw_count() {
    let (app, target) = server(bytes, ColorMode::Off).await;
    app.get("/").await;
    assert_eq!(target.next().await, "2");
}

#[test(harness)]
async fn body_len_human_renders_with_units() {
    let (app, target) = server(body_len_human, ColorMode::Off).await;
    app.get("/").await;
    assert_eq!(target.next().await, "2 bytes");
}

#[test(harness)]
async fn secure_is_two_spaces_over_plaintext() {
    let (app, target) = server(secure, ColorMode::Off).await;
    app.get("/").await;
    assert_eq!(target.next().await, "  ");
}

#[test(harness)]
async fn request_header_present_is_quoted() {
    let (app, target) = server(request_header(UserAgent), ColorMode::Off).await;
    app.get("/")
        .with_request_header(UserAgent, "secret agent")
        .await;
    assert_eq!(target.next().await, r#""secret agent""#);
}

#[test(harness)]
async fn request_header_absent_is_empty_quotes() {
    let (app, target) = server(request_header("x-absent"), ColorMode::Off).await;
    app.get("/").await;
    assert_eq!(target.next().await, r#""""#);
}

#[test(harness)]
async fn host_renders_request_authority() {
    let (app, target) = server(host, ColorMode::Off).await;
    app.get("/")
        .with_request_header(Host, "example.com:8080")
        .await;
    assert_eq!(target.next().await, "example.com:8080");
}

#[test(harness)]
async fn user_agent_present_is_quoted() {
    let (app, target) = server(user_agent, ColorMode::Off).await;
    app.get("/")
        .with_request_header(UserAgent, "secret agent")
        .await;
    assert_eq!(target.next().await, r#""secret agent""#);
}

#[test(harness)]
async fn referer_present_is_quoted() {
    let (app, target) = server(referer, ColorMode::Off).await;
    app.get("/")
        .with_request_header(Referer, "http://example.com/")
        .await;
    assert_eq!(target.next().await, r#""http://example.com/""#);
}

#[test(harness)]
async fn response_header_present_is_quoted() {
    let (app, target) = server(
        response_header(trillium::KnownHeaderName::ContentType),
        ColorMode::Off,
    )
    .await;
    app.get("/").await;
    assert_eq!(target.next().await, r#""text/plain""#);
}

#[test(harness)]
async fn response_time_is_a_duration() {
    let (app, target) = server(response_time, ColorMode::Off).await;
    app.get("/").await;
    let line = target.next().await;
    assert_ne!(line, "-");
    assert!(
        line.contains(['s', 'm', 'n', 'µ']),
        "expected a duration with a time unit, got {line:?}"
    );
}

#[test(harness)]
async fn tuple_concatenates_without_separator() {
    let (app, target) = server(("a-", method, "-b"), ColorMode::Off).await;
    app.get("/").await;
    assert_eq!(target.next().await, "a-GET-b");
}

#[test(harness)]
async fn only_one_line_per_request() {
    let (app, target) = server(method, ColorMode::Off).await;
    app.get("/").await;
    assert_eq!(target.next().await, "GET");
    assert!(target.try_next().is_none());
}
