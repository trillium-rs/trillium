use trillium_basic_auth::{BasicAuth, BasicAuthConnExt};
fn main() {
    trillium_smol::run((
        BasicAuth::new("trillium", "7r1ll1um").with_realm("rust"),
        |conn: trillium::Conn| async move {
            let username = conn.basic_auth_username().unwrap_or_default().to_string();
            conn.ok(format!("authenticated as {username}"))
        },
    ));
}
