use trillium::{Conn, Handler};

/// a struct that halts the Conn handler sequence. see [`Conn::halt`]
/// for more.
#[derive(Clone, Copy, Debug)]
pub struct Halt;

impl Handler for Halt {
    async fn run(&self, conn: Conn) -> Conn {
        conn.halt()
    }
}
