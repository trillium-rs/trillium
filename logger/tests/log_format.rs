//! Coverage for the `log_format!` macro.

mod common;

use common::server;
use trillium::{Conn, KnownHeaderName::UserAgent};
use trillium_logger::{ColorMode, formatters::request_header, log_format};
use trillium_testing::{harness, test};

fn marker(_conn: &Conn, _color: bool) -> &'static str {
    "[m]"
}

#[test(harness)]
async fn bare_names_and_literal_text() {
    let (app, target) = server(log_format!("{method} {url} -> {status}"), ColorMode::Off).await;
    app.get("/path?q=1").await;
    assert_eq!(target.next().await, "GET /path?q=1 -> 418");
}

#[test(harness)]
async fn named_args_override_with_a_builder() {
    let (app, target) = server(
        log_format!(
            "{marker} {method} {ua}",
            marker = marker,
            ua = request_header(UserAgent),
        ),
        ColorMode::Off,
    )
    .await;
    app.get("/")
        .with_request_header(UserAgent, "secret agent")
        .await;
    assert_eq!(target.next().await, r#"[m] GET "secret agent""#);
}

#[test(harness)]
async fn positional_arguments() {
    let (app, target) = server(log_format!("{} {method}", marker), ColorMode::Off).await;
    app.get("/").await;
    assert_eq!(target.next().await, "[m] GET");
}

#[test(harness)]
async fn glued_punctuation_like_apache() {
    let (app, target) = server(log_format!("\"{method} {url}\" {status}"), ColorMode::Off).await;
    app.get("/p").await;
    assert_eq!(target.next().await, r#""GET /p" 418"#);
}

#[test(harness)]
async fn escaped_braces() {
    let (app, target) = server(log_format!("{{{status}}}"), ColorMode::Off).await;
    app.get("/").await;
    assert_eq!(target.next().await, "{418}");
}

#[test(harness)]
async fn literal_only_format() {
    let (app, target) = server(log_format!("static line"), ColorMode::Off).await;
    app.get("/").await;
    assert_eq!(target.next().await, "static line");
}

#[test(harness)]
async fn color_path_contains_code() {
    let (app, target) = server(log_format!("{status}"), ColorMode::On).await;
    app.get("/").await;
    assert!(target.next().await.contains("418"));
}

// 14 placeholders + 13 separators = 27 tuple elements, exceeding the 26-arity ceiling and
// exercising the macro's tuple-nesting.
#[test(harness)]
async fn large_format_nests_past_tuple_arity() {
    let (app, target) = server(
        log_format!(
            "{method} {method} {method} {method} {method} {method} {method} {method} {method} \
             {method} {method} {method} {method} {method}"
        ),
        ColorMode::Off,
    )
    .await;
    app.get("/").await;
    assert_eq!(target.next().await, ["GET"; 14].join(" "));
}
