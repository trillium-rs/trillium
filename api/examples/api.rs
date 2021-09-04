use serde::{Deserialize, Serialize};
use trillium::Conn;
use trillium_api::{api, ApiConnExt};
use trillium_logger::Logger;

#[derive(Serialize, Deserialize, Debug)]
struct Post {
    user_id: usize,
    id: Option<usize>,
    title: String,
    body: String,
}

fn main() {
    trillium_smol::run((
        Logger::new(),
        api(|conn: Conn, mut post: Post| async move {
            post.id = Some(10);
            conn.with_json(&post)
        }),
    ));
}
