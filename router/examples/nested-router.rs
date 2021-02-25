use myco::{sequence, Conn, Handler};
use myco_router::{Router, RouterConnExt};

struct User {
    id: usize,
}

async fn load_user(conn: Conn) -> Conn {
    if let Ok(id) = conn.param("user_id").unwrap().parse() {
        let user = User { id }; // imagine we were loading a user from a database here
        conn.with_state(user)
    } else {
        conn.status(404).halt()
    }
}

fn nested_app() -> impl Handler {
    sequence![
        load_user,
        Router::new()
            .get("/greeting", |mut conn: Conn| async move {
                let user = conn.take_state::<User>().unwrap();
                conn.ok(format!("hello user {}", user.id))
            })
            .get("/some/other/route", |conn: Conn| async move {
                conn.ok("this is an uninspired example")
            })
    ]
}

pub fn main() {
    env_logger::init();
    myco_smol_server::run(
        Router::new()
            .get("/", |conn: Conn| async move { conn.ok("hello everyone") })
            .get("/users/:user_id/*", nested_app()),
    );
}
