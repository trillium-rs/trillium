use myco::{sequence, Conn};
use myco_logger::DevLogger;

pub fn main() {
    env_logger::init();
    let handler = sequence![DevLogger, |conn: Conn| async move { conn.ok("ok!") }];
    myco_smol_server::run("localhost:8000", (), handler);
}
