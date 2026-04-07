# Patterns for library authors

## The ConnExt pattern

Most Trillium libraries follow a consistent pattern for extending `Conn` with new capabilities:

1. A **handler** runs early in the chain and stores data in the conn's state set using a private newtype wrapper.
2. A **`[Something]ConnExt` trait** provides typed accessor methods on `Conn` for reading that data.

Using a private newtype ensures that only your library's handler sets that state — no other crate can accidentally overwrite it, because `StateSet` holds exactly one value per type and the type is not accessible outside your crate.

Here's a worked example: a handler that numbers each conn in order and makes that number available to downstream handlers.

### The library implementation

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
#
# fn main() {
mod conn_counter {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    use trillium::{Conn, Handler};

    struct ConnNumber(u64);

    #[derive(Default)]
    pub struct ConnCounterHandler(Arc<AtomicU64>);

    impl ConnCounterHandler {
        pub fn new() -> Self {
            Self::default()
        }
    }

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
# }
```

### Usage

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-testing = { path = "../testing" }
#
# use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
# struct ConnNumber(u64);
# #[derive(Default)]
# pub struct ConnCounterHandler(Arc<AtomicU64>);
# impl ConnCounterHandler {
#     pub fn new() -> Self { Self::default() }
# }
# impl trillium::Handler for ConnCounterHandler {
#     async fn run(&self, conn: trillium::Conn) -> trillium::Conn {
#         let number = self.0.fetch_add(1, Ordering::SeqCst);
#         conn.with_state(ConnNumber(number))
#     }
# }
# pub trait ConnCounterConnExt {
#     fn conn_number(&self) -> u64;
# }
# impl ConnCounterConnExt for trillium::Conn {
#     fn conn_number(&self) -> u64 {
#         self.state::<ConnNumber>().expect("conn_number must be called after the handler").0
#     }
# }
use std::time::Instant;
use trillium::{Conn, Handler, Init};

struct ServerStart(Instant);

fn handler() -> impl Handler {
    (
        Init::new(|info| async move { info.with_shared_state(ServerStart(Instant::now())) }),
        ConnCounterHandler::new(),
        |conn: Conn| async move {
            let uptime = conn
                .shared_state()
                .map(|ServerStart(instant)| instant.elapsed())
                .unwrap_or_default();
            let conn_number = conn.conn_number();
            conn.ok(format!(
                "conn number was {conn_number}, server has been up {uptime:?}"
            ))
        },
    )
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

## Shared server state

For data that lives at the server level (database pools, configuration, shared counters), use the `State<T>` handler from the `trillium` crate. It clones a value into the state of every `Conn` that passes through:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
#
# #[derive(Clone)]
# struct MyDbPool;
# fn main() {
# let my_db_pool = MyDbPool;
use trillium::{State, Conn};

trillium_smol::run((
    State::new(my_db_pool.clone()),
    |mut conn: Conn| async move {
        let pool = conn.take_state::<MyDbPool>().unwrap();
        // use pool...
        conn.ok("done")
    },
));
# }
```

The `Init` handler (also from `trillium`) runs an async setup function once at startup and can store data in the server-level shared state, which is then available on every `Conn` via `conn.shared_state::<T>()`. See the state.rs example above for usage.
