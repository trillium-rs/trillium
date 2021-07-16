use trillium_conn_id::*;
use trillium_testing::{prelude::*, TestConn};
use uuid::Uuid;

#[test]
fn test_defaults() {
    fastrand::seed(1000);
    let app = (ConnId::new(), "ok");

    assert_ok!(get("/").on(&app), "ok", "x-request-id" => "CBd9yySM2L");
    assert_ok!(get("/").on(&app), "ok", "x-request-id" => "SiB6BeZMpf");
    assert_ok!(
        get("/").with_request_header(("x-request-id", "inbound-id")).on(&app),
        "ok",
        "x-request-id" => "inbound-id"
    );

    let conn = get("/").on(&app);
    assert_eq!(conn.id(), "diPTBAoTAe");
    assert_eq!(log_formatter::id(&conn, true), "diPTBAoTAe");

    let conn = TestConn::build("get", "/", ());
    assert_eq!(log_formatter::id(&conn, true), "-");
}

#[test]
fn test_settings() {
    let app = (
        ConnId::new()
            .with_request_header(Some("x-custom-id"))
            .with_response_header(Some("x-something-else"))
            .with_id_generator(|| Uuid::new_v4().to_string()),
        "ok",
    );

    let mut conn = get("/").on(&app);

    assert!(Uuid::parse_str(conn.headers_mut()["x-something-else"].as_str()).is_ok());
    assert!(Uuid::parse_str(conn.id()).is_ok());
    assert!(Uuid::parse_str(log_formatter::id(&conn, true)).is_ok());

    assert_ok!(
        get("/").with_request_header(("x-custom-id", "inbound-id")).on(&app),
        "ok",
        "x-something-else" => "inbound-id"
    );
}

#[test]
fn test_no_headers() {
    fastrand::seed(1000);

    let app = (
        ConnId::new()
            .with_request_header(None)
            .with_response_header(None),
        "ok",
    );

    let mut conn = get("/").on(&app);
    assert!(conn.headers_mut().get("x-request-id").is_none());
    assert_eq!(conn.id(), "CBd9yySM2L");

    let mut conn = get("/")
        .with_request_header(("x-request-id", "ignored"))
        .on(&app);
    assert_eq!(conn.id(), "SiB6BeZMpf");
    assert!(conn.headers_mut().get("x-request-id").is_none());
}
