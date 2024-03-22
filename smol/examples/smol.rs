use trillium_smol::async_io::Timer;

pub fn app() -> impl trillium::Handler {
    (
        trillium_logger::Logger::new(),
        |conn: trillium::Conn| async move {
            let response = async_global_executor::spawn(async {
                Timer::after(std::time::Duration::from_millis(10)).await;
                "successfully spawned a task"
            })
            .await;

            conn.ok(response)
        },
    )
}
pub fn main() {
    env_logger::init();
    trillium_smol::run(app());
}

#[cfg(all(test, feature = "smol"))]
mod tests {
    use trillium_testing::prelude::*;
    #[test]
    fn test() {
        let app = super::app();
        assert_ok!(get("/").on(&app), "successfully spawned a task");
    }
}
