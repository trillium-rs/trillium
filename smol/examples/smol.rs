use std::time::Duration;
use trillium::{Conn, Handler};
use trillium_logger::Logger;
use trillium_server_common::Runtime;

pub fn app() -> impl Handler {
    (Logger::new(), |conn: Conn| async move {
        let runtime = conn.shared_state::<Runtime>().cloned().unwrap();
        let response = runtime
            .clone()
            .spawn(async move {
                runtime.delay(Duration::from_millis(100)).await;
                "successfully spawned a task"
            })
            .await
            .unwrap_or_default();

        conn.ok(response)
    })
}

pub fn main() {
    env_logger::init();
    trillium_smol::run(app());
}
