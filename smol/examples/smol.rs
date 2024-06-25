use std::time::Duration;
use trillium::{Conn, Handler};
use trillium_logger::Logger;
use trillium_smol::SmolRuntime;

pub fn app() -> impl Handler {
    (Logger::new(), |conn: Conn| async move {
        let runtime = SmolRuntime::default();
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

#[cfg(test)]
mod tests {
    use trillium_testing::prelude::*;
    #[test]
    fn test() {
        let app = super::app();
        assert_ok!(get("/").on(&app), "successfully spawned a task");
    }
}
