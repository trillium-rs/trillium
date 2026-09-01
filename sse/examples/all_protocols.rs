use broadcaster::BroadcastChannel;
use std::{process::ExitCode, time::Duration};
use trillium::{Conn, Method, State, conn_try, conn_unwrap, log_error};
use trillium_logger::logger;
use trillium_quinn::QuicConfig;
use trillium_rustls::RustlsAcceptor;
use trillium_sse::sse;
use trillium_static_compiled::static_compiled;
type Channel = BroadcastChannel<String>;

fn cert_and_key() -> Option<(Vec<u8>, Vec<u8>)> {
    let host_path = std::env::var("CERT").ok()?;
    let key_path = std::env::var("KEY").ok()?;
    let cert_file = std::fs::read(host_path).ok()?;
    let key_file = std::fs::read(key_path).ok()?;
    Some((cert_file, key_file))
}

pub fn main() -> ExitCode {
    env_logger::init();
    let Some((cert, key)) = cert_and_key() else {
        eprintln!("CERT and KEY env vars should point at files");
        return ExitCode::FAILURE;
    };

    let broadcast = Channel::new();
    let handler = (
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
    );

    trillium_smol::config()
        .with_acceptor(RustlsAcceptor::from_single_cert(&cert, &key))
        .with_quic(QuicConfig::from_single_cert(&cert, &key))
        .run(handler);

    ExitCode::SUCCESS
}

async fn post_broadcast(mut conn: Conn) -> Conn {
    let broadcaster = conn_unwrap!(conn.take_state::<Channel>(), conn);
    let body = conn_try!(conn.request_body_string().await, conn);
    log_error!(broadcaster.send(&body).await);
    conn.ok("sent")
}
