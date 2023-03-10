use trillium::{async_trait, Conn, Handler};

#[derive(Clone, Copy, Debug)]
pub struct Halt;

#[async_trait]
impl Handler for Halt {
    async fn run(&self, conn: Conn) -> Conn {
        conn.halt()
    }
}
