use trillium_method_override::*;
use trillium_testing::prelude::*;

async fn test_handler(conn: Conn) -> Conn {
    match (conn.method(), conn.path()) {
        (Method::Delete, _) => conn.ok("you did a delete"),
        (Method::Post, _) => conn.ok("it was a post"),
        (Method::Patch, _) => conn.ok("adams"),
        (Method::Put, _) => conn.ok("put and call"),
        _ => conn,
    }
}

#[test]
fn test() {
    let app = (MethodOverride::new(), test_handler);

    assert_ok!(post("/?_method=delete").on(&app), "you did a delete");
    assert_ok!(
        post("/?a=b&_method=delete&c=d").on(&app),
        "you did a delete"
    );

    assert_ok!(post("/?_method=connect").on(&app), "it was a post");
    assert_ok!(post("/?_method!!-=/=connect").on(&app), "it was a post");

    assert_not_handled!(get("/?_method=delete").on(&app));
}

#[test]
fn with_limited_allowed_methods() {
    let app = (
        MethodOverride::new().with_allowed_methods(["put", "patch"]),
        test_handler,
    );

    assert_ok!(post("/?_method=put").on(&app), "put and call");
    assert_ok!(post("/?a=b&_method=patch&c=d").on(&app), "adams");

    assert_ok!(post("/?_method=delete").on(&app), "it was a post");
}

#[test]
fn with_a_different_param_name() {
    let app = (MethodOverride::new().with_param_name("verb"), test_handler);
    assert_ok!(post("/?verb=delete").on(&app), "you did a delete");
    assert_ok!(post("/?_method=delete").on(&app), "it was a post");
}
