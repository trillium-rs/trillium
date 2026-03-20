# Patterns for library authors

## State

Let's take a look at an implementation of a library that incrementally counts the number of conns that pass through it and attaches the number to each conn. It would be unsafe to store a u64 directly in the state set, because other libraries might be doing so, so we wrap it with a private newtype called ConnNumber. Since this isn't accessible outside of our library, we can be sure that our handler is the only place that sets it.  We provide a ConnExt trait in order to provide access to this data.

```rust
mod conn_counter {
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };
    use trillium::{async_trait, Conn, Handler};

    struct ConnNumber(u64);

    #[derive(Default)]
    pub struct ConnCounterHandler(Arc<AtomicU64>);

    impl ConnCounterHandler {
        pub fn new() -> Self {
            Self::default()
        }
    }

    #[async_trait]
    impl Handler for ConnCounterHandler {
        async fn run(&self, conn: Conn) -> Conn {
            let number = self.0.fetch_add(1, Ordering::SeqCst);
            conn.with_state(ConnNumber(number))
        }
    }

    pub trait ConnCounterConnExt {
        fn conn_number(&self) -> u64;
    }

    impl ConnCounterConnExt for Conn {
        fn conn_number(&self) -> u64 {
            self.state::<ConnNumber>()
                .expect("conn_number must be called after the handler")
                .0
        }
    }
}
```

And usage of the library looks like this:


```rust
use conn_counter::{ConnCounterConnExt, ConnCounterHandler};
use trillium::{Conn, Handler};

fn handler() -> impl Handler {
    (ConnCounterHandler::new(), |conn: Conn| async move {
        let conn_number = conn.conn_number();
        conn.ok(format!("conn number was {conn_number}"))
    })
}

fn main() {
    trillium_smol::run(handler());
}

#[cfg(test)]
mod test {
    use trillium_testing::prelude::*;

    #[test]
    fn test_conn_counter() {
        let handler = super::handler();
        assert_ok!(get("/").on(&handler), "conn number was 0");
        assert_ok!(get("/").on(&handler), "conn number was 1");
        assert_ok!(get("/").on(&handler), "conn number was 2");
        assert_ok!(get("/").on(&handler), "conn number was 3");
    }
}
```
