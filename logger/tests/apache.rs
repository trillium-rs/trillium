use access_log_parser::{LogEntry, LogType, RequestResult, parse};
use std::{net::IpAddr, sync::Arc};
use time::OffsetDateTime;
use trillium::{
    KnownHeaderName::{Referer, UserAgent},
    Status, Version,
};
use trillium_logger::{ColorMode, Targetable, apache_combined, apache_common, logger};
use trillium_testing::{TestHandler, harness, test};

async fn teapot(conn: trillium::Conn) -> trillium::Conn {
    conn.with_status(Status::ImATeapot).with_body("ok")
}

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
        let sender = &self.0.0;
        sender.send_blocking(data).unwrap();
    }
}

impl TestTarget {
    async fn next(&self) -> String {
        self.0.1.recv().await.unwrap()
    }
}

#[test(harness)]
async fn test_apache_combined() {
    let target = TestTarget::default();
    let logger = logger()
        .with_formatter(apache_combined("request-id", "user"))
        .with_target(target.clone())
        .with_color_mode(ColorMode::Off);

    let ip = IpAddr::from([1, 2, 3, 4]);
    let handler = (logger, teapot);

    let app = TestHandler::new(handler).await;

    let _ = target.next().await; // startup message

    app.get("/some/path?query")
        .with_request_header(Referer, "http://example.com")
        .with_request_header(UserAgent, "secret agent")
        .with_peer_ip(ip)
        .await
        .assert_status(Status::ImATeapot)
        .assert_body("ok");

    let log_entry = target.next().await;

    let LogEntry::CombinedLog(log) = parse(LogType::CombinedLog, &log_entry).unwrap() else {
        panic!()
    };

    let RequestResult::Valid(request) = log.request else {
        panic!()
    };

    assert_eq!(log.ip, ip);
    assert!(OffsetDateTime::now_utc().unix_timestamp() - log.timestamp.timestamp() < 2);
    assert_eq!(request.uri(), "/some/path?query");
    assert_eq!(request.method(), "GET");
    assert_eq!(log.status_code, Status::ImATeapot);
    assert_eq!(request.version(), Version::Http1_1);
    assert_eq!(log.identd_user, Some("request-id"));
    assert_eq!(log.user, Some("user"));
    assert_eq!(log.referrer, Some("http://example.com".parse().unwrap()));
    assert_eq!(log.user_agent, Some("secret agent"));
}

#[test(harness)]
async fn test_apache_common() {
    let target = TestTarget::default();
    let logger = logger()
        .with_formatter(apache_common("request-id", "user"))
        .with_target(target.clone())
        .with_color_mode(ColorMode::Off);

    let ip = IpAddr::from([1, 2, 3, 4]);
    let handler = (logger, teapot);
    let app = TestHandler::new(handler).await;
    let _ = target.next().await;

    app.get("/some/path?query")
        .with_request_header(Referer, "http://example.com")
        .with_request_header(UserAgent, "secret agent")
        .with_peer_ip(ip)
        .await
        .assert_status(Status::ImATeapot)
        .assert_body("ok");

    let log_entry = target.next().await;

    let log = match parse(LogType::CommonLog, &log_entry).unwrap() {
        LogEntry::CommonLog(log) => log,
        other => panic!("unexpectd log type {other:?}"),
    };
    let RequestResult::Valid(request) = log.request else {
        panic!()
    };
    assert!(OffsetDateTime::now_utc().unix_timestamp() - log.timestamp.timestamp() < 2);
    assert_eq!(log.ip, ip);
    assert_eq!(request.uri(), "/some/path?query");
    assert_eq!(request.method(), "GET");
    assert_eq!(log.status_code, Status::ImATeapot);
    assert_eq!(request.version(), Version::Http1_1);
    assert_eq!(log.identd_user, Some("request-id"));
    assert_eq!(log.user, Some("user"));
}
