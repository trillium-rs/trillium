use std::sync::atomic::{AtomicUsize, Ordering};

use trillium_conn_id::*;
use trillium_testing::{prelude::*, TestConn};
use uuid::Uuid;

fn build_incrementing_id_generator() -> impl Fn() -> String + Send + Sync + 'static {
    let count = AtomicUsize::default();
    move || count.fetch_add(1, Ordering::SeqCst).to_string()
}

#[test]
fn test_defaults() {
    let app = (ConnId::new().with_seed(1000), "ok");
    assert_ok!(get("/").on(&app), "ok", "x-request-id" => "J4lzoPXcT5");
    assert_ok!(get("/").on(&app), "ok", "x-request-id" => "Sn0wUTe4EF");
    assert_ok!(
        get("/").with_request_header("x-request-id", "inbound-id").on(&app),
        "ok",
        "x-request-id" => "inbound-id"
    );

    let conn = get("/").on(&app);
    assert_eq!(conn.id(), "cnx2OnqZsR");
    assert_eq!(log_formatter::conn_id(&conn, true), "cnx2OnqZsR");

    let conn = TestConn::build("get", "/", ());
    assert_eq!(log_formatter::conn_id(&conn, true), "-");
}

#[test]
fn test_settings() {
    let app = (
        ConnId::new()
            .with_request_header("x-custom-id")
            .with_response_header("x-something-else")
            .with_id_generator(|| Uuid::new_v4().to_string()),
        "ok",
    );

    let conn = get("/").on(&app);

    assert!(Uuid::parse_str(conn.response_headers().get_str("x-something-else").unwrap()).is_ok());
    assert!(Uuid::parse_str(conn.id()).is_ok());
    assert!(Uuid::parse_str(&log_formatter::conn_id(&conn, true)).is_ok());

    assert_ok!(
        get("/").with_request_header("x-custom-id", "inbound-id").on(&app),
        "ok",
        "x-something-else" => "inbound-id"
    );
}

#[test]
fn test_no_headers() {
    let app = (
        ConnId::new()
            .with_id_generator(build_incrementing_id_generator())
            .without_request_header()
            .without_response_header(),
        "ok",
    );

    let conn = get("/").on(&app);
    assert!(conn.response_headers().get("x-request-id").is_none());
    assert_eq!(conn.id(), "0");

    let conn = get("/")
        .with_request_header("x-request-id", "ignored")
        .on(&app);
    assert_eq!(conn.id(), "1");
    assert!(conn.response_headers().get("x-request-id").is_none());
}
