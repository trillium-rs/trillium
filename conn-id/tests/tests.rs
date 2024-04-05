use trillium_conn_id::*;
use trillium_testing::{prelude::*, TestConn};
use uuid::Uuid;

#[test]
fn test_defaults() {
    let app = (ConnId::new().with_seed(1000), "ok");
    assert_ok!(get("/").on(&app), "ok", "x-request-id" => "4fekClhof7");
    assert_ok!(get("/").on(&app), "ok", "x-request-id" => "PAmkU1LPSe");
    assert_ok!(
        get("/").with_request_header("x-request-id", "inbound-id").on(&app),
        "ok",
        "x-request-id" => "inbound-id"
    );

    let conn = get("/").on(&app);
    assert_eq!(conn.id(), "kZTgZfbUJB");
    assert_eq!(log_formatter::conn_id(&conn, true), "kZTgZfbUJB");

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
            .with_seed(1000)
            .without_request_header()
            .without_response_header(),
        "ok",
    );

    let conn = get("/").on(&app);
    assert!(conn.response_headers().get("x-request-id").is_none());
    assert_eq!(conn.id(), "4fekClhof7");

    let conn = get("/")
        .with_request_header("x-request-id", "ignored")
        .on(&app);
    assert_eq!(conn.id(), "PAmkU1LPSe");
    assert!(conn.response_headers().get("x-request-id").is_none());
}
