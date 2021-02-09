use myco::{sequence, Conn};
use myco_router::{Router, RouterConnExt};
use std::collections::HashMap;
type Database = HashMap<String, String>;
pub fn main() {
    let handler = Router
        .new()
        .get("/:key", |conn: Conn| async move {
            let key = conn.param("key").unwrap();
            let database = conn.state::<Database>().unwrap();
        })
        .put("/:key", |conn: Conn| async move {
            let key = conn.param("key").unwrap();
            let database = conn.state::<Database>().unwrap();
        });

    myco_smol_server::run("localhost:8000", (), handler);
}
