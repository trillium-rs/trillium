use myco::Conn;
use std::time::Duration;

pub fn main() {
    env_logger::init();
    myco_smol_server::run(|conn: Conn| async move {
        smol::Timer::after(Duration::from_secs(1)).await;
        conn.ok("hello")
    });
}
