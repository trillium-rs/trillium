# Sessions

[rustdocs](https://docs.trillium.rs/trillium_sessions)

Sessions associate server-side data with a browser client using a secure cookie as a key. The `trillium-sessions` crate provides a `Sessions` handler and a `SessionConnExt` trait that extends `Conn` with session read and write methods.

> ❗ The session handler depends on cookies. Place `Cookies::new()` earlier in the handler chain, before `Sessions`.

```rust
use trillium::Conn;
use trillium_cookies::CookiesHandler;
use trillium_sessions::{MemoryStore, SessionConnExt, SessionHandler};

pub fn main() {
    env_logger::init();

    trillium_smol::run((
        CookiesHandler::new(),
        SessionHandler::new(MemoryStore::new(), "01234567890123456789012345678901123"),
        |conn: Conn| async move {
            let count: usize = conn.session().get("count").unwrap_or_default();
            conn.with_session("count", count + 1)
                .ok(format!("count: {count}"))
        },
    ));
}
```

## Session stores

The session store determines where session data is persisted. Choose based on your deployment:

| Store | Notes |
|-------|-------|
| `MemoryStore` (built-in) | In-process only. All sessions are lost on restart. Fine for development. |
| `CookieStore` (built-in) | Stores all session data in the cookie itself. No server-side storage, but increases cookie size and makes server-side invalidation impossible. |
| Database stores | Recommended for production. External crates provide PostgreSQL, SQLite, Redis, and MongoDB backends. |

For database-backed stores, search crates.io for `trillium-session` or check the session crate's documentation for recommended options.
