use std::time::Duration;

use smol::Timer;
use trillium::Conn;
use trillium_logger::TracingHandler;

pub fn main() {
    tracing_subscriber::fmt::init();
    trillium_smol::run((TracingHandler::new(), |conn: Conn| async move {
        Timer::after(Duration::from_secs(1)).await;
        conn.ok("ok")
    }));
}
