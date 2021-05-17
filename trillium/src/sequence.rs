use crate::{async_trait, Conn, Handler, Upgrade};
use std::borrow::Cow;
/**
# A collection of handlers that are called serially.

A sequence is the trillium equivalent of a [Plug
pipeline](https://hexdocs.pm/plug/Plug.html#module-the-plug-pipeline),
and represents a higher order Handler which composes arbitrary
Handlers.

What would be represented in other frameworks as a middleware stack is
represented as a `Sequence` in trillium.

There are several ways to construct a sequence.

# Chained `then` calls
```
use trillium::{Sequence, Conn};
trillium_testing::server::run(
    Sequence::new()
        .then(trillium_logger::DevLogger)
        .then(|conn: Conn| async move { conn.ok("okeydokey") })
);
```
# Imperatively, with `push`

```
use trillium::{Sequence, Conn};
let mut sequence = Sequence::new();

if let Ok("yup") = std::env::var("ENABLE_TRILLIUM_LOGGER").as_deref() {
    sequence.push(trillium_logger::DevLogger);
}

sequence.push(|conn: Conn| async move { conn.ok("okeydokey") });

trillium_testing::server::run(sequence);

```


# Using the `trillium::sequence` macro

See also [`trillium::sequence`][crate::sequence]. Most trillium docs
and examples use this macro.

```
use trillium::{sequence, Conn};
trillium_testing::server::run(sequence![
    trillium_logger::DevLogger,
    |conn: Conn| async move { conn.ok("okeydokey") }
]);
```


*/
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
    /// Start a new sequence. This is a valid handler, although it
    /// will not do anything until additional handlers are added to
    /// it.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a handler to the sequence imperatively
    pub fn push(&mut self, handler: impl Handler) {
        self.0.push(Box::new(handler));
    }

    /// Chain an additional handler onto this sequence, returning the
    /// sequence.
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
