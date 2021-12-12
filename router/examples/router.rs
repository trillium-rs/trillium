use trillium::{Conn, Init};
use trillium_router::{Router, RouterConnExt};

pub fn main() {
    env_logger::init();
    trillium_smol::run(
        Router::new()
            .get(
                "/stuff",
                Init::new(|info| async move {
                    dbg!(&info);
                    "hellO"
                }),
            )
            .get("/", |conn: Conn| async move { conn.ok("hello everyone") })
            .get("/hello/:planet", |conn: Conn| async move {
                let planet = conn.param("planet").unwrap();
                let response_body = format!("hello {}", planet);
                conn.ok(response_body)
            }),
    );
}
