use trillium_basic_auth::BasicAuth;
fn main() {
    trillium_smol::run((
        BasicAuth::new("trillium", "7r1ll1um").with_realm("rust"),
        |conn: trillium::Conn| async move { conn.ok("authenticated") },
    ));
}
