use trillium::Conn;
use trillium_router::{RouterConnExt, routes};

pub fn main() {
    env_logger::init();
    trillium_smol::run(routes![
        get "/" |conn: Conn| async move { conn.ok("hello everyone") },

        get "/hello/:planet" |conn: Conn| async move {
            let planet = conn.param("planet").unwrap();
            let response_body = format!("hello {planet}");
            conn.ok(response_body)
        }
    ]);
}
