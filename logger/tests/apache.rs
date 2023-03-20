use access_log_parser::{parse, LogEntry, LogType, RequestResult};
use std::sync::{Arc, Mutex};
use time::OffsetDateTime;
use trillium::{
    KnownHeaderName::{Referer, UserAgent},
    Status, Version,
};
use trillium_logger::{apache_combined, apache_common, logger, ColorMode};
use trillium_testing::prelude::*;

async fn teapot(conn: trillium::Conn) -> trillium::Conn {
    conn.with_status(Status::ImATeapot).with_body("ok")
}

#[test]
fn test_apache_combined() {
    let s = Arc::new(Mutex::new(String::new()));
    let logger = logger()
        .with_formatter(apache_combined("request-id", "user"))
        .with_target({
            let s = s.clone();
            move |line: String| s.lock().unwrap().push_str(&line)
        })
        .with_color_mode(ColorMode::Off);

    let ip = "1.2.3.4".parse().unwrap();
    let handler = (logger, teapot);
    get("/some/path?query")
        .with_peer_ip(ip)
        .with_request_header(Referer, "http://example.com")
        .with_request_header(UserAgent, "secret agent")
        .on(&handler);
    let s = s.lock().unwrap();
    let Ok(LogEntry::CombinedLog(log)) = parse(LogType::CombinedLog, &s) else { panic!() };
    let RequestResult::Valid(request) = log.request else { panic!() };
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

#[test]
fn test_apache_common() {
    let s = Arc::new(Mutex::new(String::new()));
    let logger = logger()
        .with_formatter(apache_common("request-id", "user"))
        .with_target({
            let s = s.clone();
            move |line: String| s.lock().unwrap().push_str(&line)
        })
        .with_color_mode(ColorMode::Off);

    let ip = "1.2.3.4".parse().unwrap();
    let handler = (logger, teapot);
    get("/some/path?query")
        .with_peer_ip(ip)
        .with_request_header(Referer, "http://example.com")
        .with_request_header(UserAgent, "secret agent")
        .on(&handler);
    let s = s.lock().unwrap();
    let Ok(LogEntry::CommonLog(log)) = parse(LogType::CommonLog, &s) else { panic!() };
    let RequestResult::Valid(request) = log.request else { panic!() };
    assert!(OffsetDateTime::now_utc().unix_timestamp() - log.timestamp.timestamp() < 2);
    assert_eq!(log.ip, ip);
    assert_eq!(request.uri(), "/some/path?query");
    assert_eq!(request.method(), "GET");
    assert_eq!(log.status_code, Status::ImATeapot);
    assert_eq!(request.version(), Version::Http1_1);
    assert_eq!(log.identd_user, Some("request-id"));
    assert_eq!(log.user, Some("user"));
}
