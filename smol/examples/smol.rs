use trillium_smol::{async_global_executor, async_io::Timer};
pub fn app() -> impl trillium::Handler {
    |conn: trillium::Conn| async move {
        let response = async_global_executor::spawn(async {
            Timer::after(std::time::Duration::from_millis(10)).await;
            "successfully spawned a task"
        })
        .await;

        conn.ok(response)
    }
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
