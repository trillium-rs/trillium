use trillium_conn_id::*;
use trillium_testing::{prelude::*, TestConn};
use uuid::Uuid;

#[test]
fn test_defaults() {
    fastrand::seed(1000);
    let app = (ConnId::new(), "ok");

    assert_ok!(get("/").on(&app), "ok", "x-request-id" => "U14baHj9ho");
    assert_ok!(get("/").on(&app), "ok", "x-request-id" => "AawiNNFjGW");
    assert_ok!(
        get("/").with_request_header("x-request-id", "inbound-id").on(&app),
        "ok",
        "x-request-id" => "inbound-id"
    );

    let conn = get("/").on(&app);
    assert_eq!(conn.id(), "iHxXDjwzU5");
    assert_eq!(log_formatter::conn_id(&conn, true), "iHxXDjwzU5");

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

    let mut conn = get("/").on(&app);

    assert!(Uuid::parse_str(conn.headers_mut().get_str("x-something-else").unwrap()).is_ok());
    assert!(Uuid::parse_str(conn.id()).is_ok());
    assert!(Uuid::parse_str(&*log_formatter::conn_id(&conn, true)).is_ok());

    assert_ok!(
        get("/").with_request_header("x-custom-id", "inbound-id").on(&app),
        "ok",
        "x-something-else" => "inbound-id"
    );
}

#[test]
fn test_no_headers() {
    fastrand::seed(1000);

    let app = (
        ConnId::new()
            .without_request_header()
            .without_response_header(),
        "ok",
    );

    let mut conn = get("/").on(&app);
    assert!(conn.headers_mut().get("x-request-id").is_none());
    assert_eq!(conn.id(), "U14baHj9ho");

    let mut conn = get("/")
        .with_request_header("x-request-id", "ignored")
        .on(&app);
    assert_eq!(conn.id(), "AawiNNFjGW");
    assert!(conn.headers_mut().get("x-request-id").is_none());
}
