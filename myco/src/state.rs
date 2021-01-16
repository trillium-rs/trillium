use crate::{async_trait, Conn, Grain};
pub struct State<T>(T);

impl<T> State<T> {
    pub fn new(t: T) -> Self {
        Self(t)
    }
}
#[async_trait]
impl<T: Clone + Send + Sync + 'static> Grain for State<T> {
    async fn run(&self, mut conn: Conn) -> Conn {
        conn.set_state(self.0.clone());
        conn
    }
}
