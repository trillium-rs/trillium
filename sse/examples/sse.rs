use broadcaster::BroadcastChannel;
use std::time::Duration;
use trillium::{Conn, Method, State, conn_try, conn_unwrap, log_error};
use trillium_logger::logger;
use trillium_sse::sse;
use trillium_static_compiled::static_compiled;
type Channel = BroadcastChannel<String>;

fn main() {
    let broadcast = Channel::new();
    trillium_smol::run((
        logger(),
        static_compiled!("$CARGO_MANIFEST_DIR/examples/static").with_index_file("index.html"),
        State::new(broadcast.clone()),
        |conn: Conn| async move {
            match (conn.method(), conn.path()) {
                (Method::Post, "/broadcast") => post_broadcast(conn).await,
                _ => conn,
            }
        },
        sse(move |_: &mut Conn| broadcast.clone()).with_heartbeat(Duration::from_secs(15)),
    ));
}

async fn post_broadcast(mut conn: Conn) -> Conn {
    let broadcaster = conn_unwrap!(conn.take_state::<Channel>(), conn);
    let body = conn_try!(conn.request_body_string().await, conn);
    log_error!(broadcaster.send(&body).await);
    conn.ok("sent")
}
