mod conn_counter {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    use trillium::{Conn, Handler};

    struct ConnNumber(u64);

    #[derive(Default)]
    pub struct ConnCounterHandler(Arc<AtomicU64>);

    impl ConnCounterHandler {
        pub fn new() -> Self {
            Self::default()
        }
    }

    impl Handler for ConnCounterHandler {
        async fn run(&self, conn: Conn) -> Conn {
            let number = self.0.fetch_add(1, Ordering::SeqCst);
            conn.with_state(ConnNumber(number))
        }
    }

    pub trait ConnCounterConnExt {
        fn conn_number(&self) -> u64;
    }

    impl ConnCounterConnExt for Conn {
        fn conn_number(&self) -> u64 {
            self.state::<ConnNumber>()
                .expect("conn_number must be called after the handler")
                .0
        }
    }
}

use conn_counter::{ConnCounterConnExt, ConnCounterHandler};
use std::time::Instant;
use trillium::{Conn, Handler, Init};

struct ServerStart(Instant);

fn handler() -> impl Handler {
    (
        Init::new(|info| async move { info.with_shared_state(ServerStart(Instant::now())) }),
        ConnCounterHandler::new(),
        |conn: Conn| async move {
            let uptime = conn
                .shared_state()
                .map(|ServerStart(instant)| instant.elapsed())
                .unwrap_or_default();
            let conn_number = conn.conn_number();
            conn.ok(format!(
                "conn number was {conn_number}, server has been up {uptime:?}"
            ))
        },
    )
}

fn main() {
    trillium_smol::run(handler());
}

#[cfg(test)]
mod test {
    use trillium_testing::{TestServer, harness, test};

    #[test(harness)]
    async fn test_conn_counter() {
        let handler = TestServer::new(super::handler()).await;
        handler.get("/").await.assert_body("conn number was 0");
        handler.get("/").await.assert_body("conn number was 1");
        handler.get("/").await.assert_body("conn number was 2");
        handler.get("/").await.assert_body("conn number was 3");
    }
}
