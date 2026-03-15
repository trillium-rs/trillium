# Patterns for library authors

## The ConnExt pattern

Most Trillium libraries follow a consistent pattern for extending `Conn` with new capabilities:

1. A **handler** runs early in the chain and stores data in the conn's state set using a private newtype wrapper.
2. A **`[Something]ConnExt` trait** provides typed accessor methods on `Conn` for reading that data.

Using a private newtype ensures that only your library's handler sets that state — no other crate can accidentally overwrite it, because `StateSet` holds exactly one value per type and the type is not accessible outside your crate.

Here's a worked example: a handler that numbers each conn in order and makes that number available to downstream handlers.

### The library implementation

```rust,noplaypen
{{#include ../../trillium/examples/state.rs:1:38}}
```

### Usage

```rust,noplaypen
{{#include ../../trillium/examples/state.rs:40:}}
```

## Shared server state

For data that lives at the server level (database pools, configuration, shared counters), use the `State<T>` handler from the `trillium` crate. It clones a value into the state of every `Conn` that passes through:

```rust,noplaypen
use trillium::State;

trillium_smol::run((
    State::new(my_db_pool.clone()),
    |mut conn: Conn| async move {
        let pool = conn.take_state::<MyDbPool>().unwrap();
        // use pool...
        conn.ok("done")
    },
));
```

The `Init` handler (also from `trillium`) runs an async setup function once at startup and can store data in the server-level shared state, which is then available on every `Conn` via `conn.shared_state::<T>()`. See the state.rs example above for usage.
