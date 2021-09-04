use trillium_basic_auth::*;
use trillium_testing::prelude::*;

#[test]
fn correct_auth() {
    let handler = (BasicAuth::new("jacob", "7r1ll1um"), "ok");
    assert_ok!(
        get("/")
            .with_request_header(
                "Authorization",
                format!("Basic {}", base64::encode("jacob:7r1ll1um"))
            )
            .on(&handler),
        "ok"
    );
}

#[test]
fn incorrect_auth() {
    let handler = (BasicAuth::new("jacob", "7r1ll1um"), "ok");
    assert_response!(
        get("/")
            .with_request_header(
                "Authorization",
                format!("Basic {}", base64::encode("jacob:31337"))
            )
            .on(&handler),
        401, "", "www-authenticate" => "Basic"
    );
}

#[test]
fn incorrect_auth_with_realm() {
    let handler = (
        BasicAuth::new("gunter", "quack").with_realm("kingdom of ooo"),
        "ok",
    );
    assert_response!(
        get("/")
            .with_request_header(
                "Authorization",
                format!("Basic {}", base64::encode("orgalorg:31337"))
            )
            .on(&handler),
        401, "", "www-authenticate" => "Basic realm=\"kingdom of ooo\""
    );
}

#[test]
fn incorrect_auth_with_realm_that_includes_a_quote() {
    let handler = (
        BasicAuth::new("gunter", "quack").with_realm("kingdom of \"ooo\""),
        "ok",
    );
    assert_response!(
        get("/")
            .with_request_header(
                "Authorization",
                format!("Basic {}", base64::encode("orgalorg:31337"))
            )
            .on(&handler),
        401, "", "www-authenticate" => r#"Basic realm="kingdom of \"ooo\"""#
    );
}

#[test]
fn edge_cases() {
    let handler = (BasicAuth::new("jacob", "7r1ll1um"), "ok");
    assert_status!(
        get("/")
            .with_request_header(
                "Authorization",
                format!("Basic {}", base64::encode("jacob:"))
            )
            .on(&handler),
        401
    );

    assert_status!(
        get("/")
            .with_request_header(
                "Authorization",
                format!("Basic {}", base64::encode(":7r1ll1um"))
            )
            .on(&handler),
        401
    );

    assert_status!(
        get("/")
            .with_request_header("Authorization", format!("Basic {}", base64::encode(":")))
            .on(&handler),
        401
    );
}
