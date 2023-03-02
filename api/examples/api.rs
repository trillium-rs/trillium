use serde::{Deserialize, Serialize};
use trillium::Conn;
use trillium_api::{api, api_with_body, Json};
use trillium_logger::logger;
use trillium_router::router;

#[derive(Serialize, Deserialize, Debug)]
struct Post {
    user_id: usize,
    id: Option<usize>,
    title: String,
    body: String,
}

async fn save_post(_conn: &mut Conn, mut post: Post) -> Json<Post> {
    post.id = Some(10);
    Json(post)
}

async fn get_post(_conn: &mut Conn) -> Json<Post> {
    Json(Post {
        user_id: 10,
        id: Some(10),
        title: "post title".into(),
        body: "body".into(),
    })
}

fn main() {
    env_logger::init();
    trillium_smol::run((
        logger(),
        router()
            .post("/", api_with_body(save_post))
            .get("/", api(get_post)),
    ));
}
