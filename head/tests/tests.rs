use trillium_head::*;
use trillium_testing::{TestConn, prelude::*};

#[test]
fn test() {
    let app = (Head::new(), |conn: Conn| async move {
        match (conn.method(), conn.path()) {
            (Method::Get, "/") => conn.ok("ok, this is my body"),
            (Method::Get, _) => conn.with_status(404).with_body("egads i don't have that"),
            _ => conn,
        }
    });

    assert_ok!(TestConn::build(Method::Head, "/", ()).on(&app), "", "content-length" => "19");
    assert_ok!(get("/").on(&app), "ok, this is my body", "content-length" => "19");

    assert_response!(TestConn::build(Method::Head, "/not_found", ()).on(&app), 404, "", "content-length" => "23");
    assert_response!(get("/not_found").on(&app), 404, "egads i don't have that", "content-length" => "23");

    assert_not_handled!(post("/").on(&app));
}
