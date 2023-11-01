use trillium::{Conn, Handler};
use trillium_macros::Handler;

/// Run a [`trillium::Handler`] in `before_send`
///
/// Wrap a handler with this type to call its `run` callback in `before_send` allowing it to execute
/// even if the conn has been halted. Note that it will be called later in the handler sequence as a
/// result of this.
#[derive(Debug, Handler)]
pub struct BeforeSend<H>(#[handler(except = [run, before_send])] pub H);
impl<H: Handler> BeforeSend<H> {
    async fn run(&self, conn: Conn) -> Conn {
        conn
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        let halted = conn.is_halted();
        conn.set_halted(false);
        let mut conn = self.0.run(conn).await;
        conn.set_halted(halted || conn.is_halted());
        self.0.before_send(conn).await
    }
}
