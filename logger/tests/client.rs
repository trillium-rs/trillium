//! Tests for the client-side `ClientLogger` handler.

#![cfg(feature = "client")]

use std::sync::Arc;
use trillium::Conn as ServerConn;
use trillium_client::{Client, ClientHandler, ConnExt, KnownHeaderName, Result, Status};
use trillium_logger::{
    ColorMode, Targetable,
    client::{ClientLogger, client_log_format, formatters},
};
use trillium_testing::{ServerConnector, TestResult, harness, test};

#[derive(Clone, Debug)]
struct TestTarget(
    Arc<(
        async_channel::Sender<String>,
        async_channel::Receiver<String>,
    )>,
);

impl Default for TestTarget {
    fn default() -> Self {
        Self(Arc::new(async_channel::unbounded()))
    }
}

impl Targetable for TestTarget {
    fn write(&self, data: String) {
        self.0.0.send_blocking(data).unwrap();
    }
}

impl TestTarget {
    async fn next(&self) -> String {
        self.0.1.recv().await.unwrap()
    }

    fn try_next(&self) -> Option<String> {
        self.0.1.try_recv().ok()
    }
}

async fn teapot(conn: ServerConn) -> ServerConn {
    conn.with_status(Status::ImATeapot)
        .with_response_header(KnownHeaderName::ContentType, "text/plain")
        .with_body("ok")
}

#[test(harness)]
async fn dev_formatter_logs_after_a_request() -> TestResult {
    let target = TestTarget::default();
    let logger = ClientLogger::new()
        .with_target(target.clone())
        .with_color_mode(ColorMode::Off);

    let client = Client::new(ServerConnector::new(teapot)).with_handler(logger);

    let conn = client.get("http://example.com/widgets?id=1").await?;
    assert_eq!(conn.status(), Some(Status::ImATeapot));

    let line = target.next().await;
    // Format: "<version> <method> <url> <status> <duration>"
    assert!(line.starts_with("HTTP/1.1 GET http://example.com/widgets?id=1 418 "));
    Ok(())
}

#[test(harness)]
async fn custom_formatter_composes() -> TestResult {
    let target = TestTarget::default();
    let logger = ClientLogger::new()
        .with_target(target.clone())
        .with_color_mode(ColorMode::Off)
        .with_formatter((
            formatters::method,
            " ",
            formatters::url,
            " -> ",
            formatters::status,
            " (",
            formatters::response_header(KnownHeaderName::ContentType),
            ")",
        ));

    let client = Client::new(ServerConnector::new(teapot)).with_handler(logger);
    let _ = client.get("http://example.com/").await?;

    let line = target.next().await;
    assert_eq!(line, r#"GET http://example.com/ -> 418 ("text/plain")"#);
    Ok(())
}

/// Logger composed *before* a halting handler still logs the synthetic response.
#[test(harness)]
async fn logger_records_halted_response() -> TestResult {
    #[derive(Debug)]
    struct CacheHit;
    impl ClientHandler for CacheHit {
        async fn run(&self, conn: &mut trillium_client::Conn) -> Result<()> {
            conn.set_status(Status::Ok)
                .set_response_body("from cache")
                .halt();
            Ok(())
        }
    }

    let target = TestTarget::default();
    let logger = ClientLogger::new()
        .with_target(target.clone())
        .with_color_mode(ColorMode::Off)
        .with_formatter((
            formatters::method,
            " ",
            formatters::url,
            " ",
            formatters::status,
        ));

    // Logger first, then CacheHit. After CacheHit halts, after_response runs in reverse: CacheHit
    // first (no-op), then logger (writes the synthetic response).
    let client = Client::new(ServerConnector::new(teapot)).with_handler((logger, CacheHit));

    let conn = client.get("http://example.com/cached").await?;
    assert_eq!(conn.status(), Some(Status::Ok));

    let line = target.next().await;
    assert_eq!(line, "GET http://example.com/cached 200");
    Ok(())
}

#[test(harness)]
async fn response_time_renders_a_duration() -> TestResult {
    let target = TestTarget::default();
    let logger = ClientLogger::new()
        .with_target(target.clone())
        .with_color_mode(ColorMode::Off)
        .with_formatter(formatters::response_time);

    let client = Client::new(ServerConnector::new(teapot)).with_handler(logger);
    let _ = client.get("http://example.com/").await?;

    let line = target.next().await;
    assert_ne!(
        line, "-",
        "expected an actual duration, got the unstamped fallback"
    );
    Ok(())
}

#[test(harness)]
async fn no_log_line_before_request_completes() -> TestResult {
    let target = TestTarget::default();
    let logger = ClientLogger::new()
        .with_target(target.clone())
        .with_color_mode(ColorMode::Off);

    let _client = Client::new(ServerConnector::new(teapot)).with_handler(logger);
    assert!(target.try_next().is_none());
    Ok(())
}

#[test(harness)]
async fn macro_bare_names_and_named_args() -> TestResult {
    let target = TestTarget::default();
    let logger = ClientLogger::new()
        .with_target(target.clone())
        .with_color_mode(ColorMode::Off)
        .with_formatter(client_log_format!(
            "{method} {url} -> {status} ({ct})",
            ct = formatters::response_header(KnownHeaderName::ContentType),
        ));

    let client = Client::new(ServerConnector::new(teapot)).with_handler(logger);
    let _ = client.get("http://example.com/widgets").await?;

    assert_eq!(
        target.next().await,
        r#"GET http://example.com/widgets -> 418 ("text/plain")"#
    );
    Ok(())
}

#[test(harness)]
async fn secure_formatter_marks_tls_schemes() -> TestResult {
    let target = TestTarget::default();
    let logger = ClientLogger::new()
        .with_target(target.clone())
        .with_color_mode(ColorMode::Off)
        .with_formatter(formatters::secure);

    let client = Client::new(ServerConnector::new(teapot)).with_handler(logger);
    let _ = client.get("https://example.com/").await?;

    assert_eq!(target.next().await, "🔒");
    Ok(())
}

#[test(harness)]
async fn peer_addr_renders_dash_without_an_address() -> TestResult {
    let target = TestTarget::default();
    let logger = ClientLogger::new()
        .with_target(target.clone())
        .with_color_mode(ColorMode::Off)
        .with_formatter(formatters::peer_addr);

    let client = Client::new(ServerConnector::new(teapot)).with_handler(logger);
    let _ = client.get("http://example.com/").await?;

    assert_eq!(target.next().await, "-");
    Ok(())
}

#[test(harness)]
async fn bytes_uses_content_length() -> TestResult {
    let target = TestTarget::default();
    let logger = ClientLogger::new()
        .with_target(target.clone())
        .with_color_mode(ColorMode::Off)
        .with_formatter(formatters::bytes);

    let client = Client::new(ServerConnector::new(teapot)).with_handler(logger);
    let _ = client.get("http://example.com/").await?;

    assert_eq!(target.next().await, "2");
    Ok(())
}
