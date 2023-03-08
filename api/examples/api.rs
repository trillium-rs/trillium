use serde::{Deserialize, Serialize};
use trillium::Conn;
use trillium_api::{api, Body};
use trillium_logger::logger;
use trillium_router::router;

#[derive(Clone, Copy, Debug)]
struct MyState;

#[derive(Serialize, Deserialize, Debug)]
struct Post {
    user_id: String,
    id: Option<usize>,
    title: String,
    body: String,
}

async fn save_post(_conn: &mut Conn, mut post: Body<Post>) -> Body<Post> {
    post.id = Some(10);
    post
}

async fn get_post(_conn: &mut Conn, _: ()) -> Body<Post> {
    Body(Post {
        user_id: "trillium@trillium.rs".into(),
        id: Some(10),
        title: "post title".into(),
        body: "body".into(),
    })
}

fn main() {
    env_logger::init();
    trillium_smol::run((
        logger(),
        router().post("/", api(save_post)).get("/", api(get_post)),
    ));
}
