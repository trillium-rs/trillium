use trillium::Conn;
use trillium_sessions::{MemoryStore, SessionConnExt, Sessions};

pub fn main() {
    env_logger::init();

    trillium_smol_server::run((
        trillium_cookies::Cookies,
        Sessions::new(MemoryStore::new(), b"01234567890123456789012345678901123"),
        |conn: Conn| async move {
            let count: usize = conn.session().get("count").unwrap_or_default();
            conn.with_session("count", count + 1)
                .ok(format!("count: {}", count))
        },
    ));
}
