use trillium::{conn_try, conn_unwrap, Conn, Handler};
use trillium_logger::Logger;
use trillium_router::{Router, RouterConnExt};

struct User {
    id: usize,
}

mod nested_app {
    use super::*;
    async fn load_user(conn: Conn) -> Conn {
        let id = conn_try!(conn, conn.param("user_id").unwrap().parse());
        let user = User { id }; // imagine we were loading a user from a database here
        conn.with_state(user)
    }

    async fn greeting(mut conn: Conn) -> Conn {
        let user = conn_unwrap!(conn, conn.take_state::<User>());
        conn.ok(format!("hello user {}", user.id))
    }

    async fn post(mut conn: Conn) -> Conn {
        let user = conn_unwrap!(conn, conn.take_state::<User>());
        let body = conn_try!(conn, conn.request_body_string().await);
        conn.ok(format!("hello user {}, {}", user.id, body))
    }

    async fn some_other_route(conn: Conn) -> Conn {
        conn.ok("this is an uninspired example")
    }

    pub fn handler() -> impl Handler {
        (
            load_user,
            Router::new()
                .get("/greeting", greeting)
                .get("/some/other/route", some_other_route)
                .post("/post", post),
        )
    }
}
pub fn main() {
    env_logger::init();
    trillium_smol::run((
        Logger::new(),
        Router::new()
            .get("/", |conn: Conn| async move { conn.ok("hello everyone") })
            .any("/users/:user_id/*", nested_app::handler()),
    ));
}
