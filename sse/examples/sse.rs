use broadcaster::BroadcastChannel;
use trillium::{Conn, Method, State, conn_try, conn_unwrap, log_error};
use trillium_logger::logger;
use trillium_sse::SseConnExt;
use trillium_static_compiled::static_compiled;
type Channel = BroadcastChannel<String>;

fn main() {
    let broadcast = Channel::new();
    trillium_smol::run((
        logger(),
        static_compiled!("$CARGO_MANIFEST_DIR/examples/static").with_index_file("index.html"),
        State::new(broadcast),
        |conn: Conn| async move {
            match (conn.method(), conn.path()) {
                (Method::Get, "/sse") => get_sse(conn),
                (Method::Post, "/broadcast") => post_broadcast(conn).await,
                _ => conn,
            }
        },
    ));
}

fn get_sse(mut conn: Conn) -> Conn {
    let broadcaster = conn_unwrap!(conn.take_state::<Channel>(), conn);
    conn.with_sse_stream(broadcaster)
}

async fn post_broadcast(mut conn: Conn) -> Conn {
    let broadcaster = conn_unwrap!(conn.take_state::<Channel>(), conn);
    let body = conn_try!(conn.request_body_string().await, conn);
    log_error!(broadcaster.send(&body).await);
    conn.ok("sent")
}
