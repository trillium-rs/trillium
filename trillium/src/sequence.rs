use crate::{async_trait, Conn, Handler, Upgrade};
use std::borrow::Cow;
#[derive(Default)]
pub struct Sequence(Vec<Box<dyn Handler>>);

impl std::fmt::Debug for Sequence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("sequence!")?;
        f.debug_list()
            .entries(self.0.iter().map(|h| h.name()))
            .finish()
    }
}

impl Sequence {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, handler: impl Handler) {
        self.0.push(Box::new(handler));
    }

    pub fn then(mut self, handler: impl Handler) -> Self {
        self.push(handler);
        self
    }
}

#[async_trait]
impl Handler for Sequence {
    async fn run(&self, conn: Conn) -> Conn {
        self.0.run(conn).await
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        self.0.before_send(conn).await
    }
    async fn init(&mut self) {
        self.0.init().await
    }

    fn name(&self) -> Cow<'static, str> {
        self.0.name()
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        self.0.has_upgrade(upgrade)
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        self.0.upgrade(upgrade).await
    }
}

#[cfg(test)]
mod tests {
    use super::Sequence;
    #[test]
    fn sequence_debug() {
        let sequence = Sequence::new();
        assert_eq!(format!("{:?}", sequence), "sequence![]");

        let sequence = Sequence::new().then("hello").then("world");
        assert_eq!(format!("{:?}", sequence), "sequence![\"hello\", \"world\"]");

        let sequence = crate::sequence!["hello", "world"];
        assert_eq!(format!("{:?}", sequence), "sequence![\"hello\", \"world\"]");
    }
}
