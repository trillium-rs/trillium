use serde::{Deserialize, Serialize};
use trillium::{Conn, Status};
use trillium_api::{Body, Halt, Result, State, api, missing_handler};
use trillium_logger::logger;
use trillium_router::router;

#[derive(Serialize, Deserialize, Debug)]
struct Post {
    user_name: String,
    id: Option<usize>,
    title: String,
    body: String,
}

async fn save_post(
    _conn: &mut Conn,
    (State(user), post): (State<User>, Result<Body<Post>>),
) -> Result<Body<Post>> {
    post.map(|mut post| {
        post.id = Some(10);
        post.user_name = user.name;
        post
    })
}

async fn save_post_alternative(
    _conn: &mut Conn,
    (State(user), mut post): (State<User>, Body<Post>),
) -> Body<Post> {
    post.id = Some(10);
    post.user_name = user.name;
    post
}

async fn get_post(_conn: &mut Conn, State(user): State<User>) -> Body<Post> {
    Body(Post {
        user_name: user.name.to_string(),
        id: Some(10),
        title: "post title".into(),
        body: "body".into(),
    })
}

#[derive(Clone)]
struct User {
    name: String,
}

async fn very_securely_set_user(conn: &mut Conn, _: ()) -> Option<State<User>> {
    conn.request_headers().get_str("x-username").map(|name| {
        State(User {
            name: name.to_string(),
        })
    })
}

fn main() {
    env_logger::init();
    trillium_smol::run((
        logger(),
        missing_handler((Status::InternalServerError, Halt)),
        api(very_securely_set_user),
        router()
            .post("/", api(save_post))
            .get("/", api(get_post))
            .put("/", api(save_post_alternative)),
    ));
}
