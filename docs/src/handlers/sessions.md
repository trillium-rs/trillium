## Sessions

[rustdocs (main)](https://docs.trillium.rs/trillium_sessions/index.html)

Sessions are a common convention in web frameworks, allowing for a
safe and secure way to associate server-side data with a given http
client (browser). Trillium's session storage is built on the
`async-session` crate, which allows us to share session stores with
tide. Currently, these session stores exist:

* MemoryStore (reexported as trillium_sessions::MemoryStore) [^1]
* CookieStore (reexported as trillium_sessions::CookieStore) [^1]
* PostgresSessionStore and SqliteSessionStore from [async-sqlx-session](https://github.com/jbr/async-sqlx-session)
* RedisSessionStore from [async-redis-session](https://github.com/jbr/async-redis-session)
* MongodbSessionStore from [async-mongodb-session](https://github.com/http-rs/async-mongodb-session)

[^1]: The memory store and cookie store should be avoided for use in
    production applications. The memory store will lose all session
    state on server process restart, and the cookie store makes
    different security tradeoffs than the database-backed stores. If
    possible, use a database.

> ❗The session handler _must_ be used in conjunction with the cookie
> handler, and it must run _after_ the cookie handler. This particular
> interaction is also present in other frameworks, and is due to the
> fact that regardless of which session store is used, sessions use a
> secure cookie as a unique identifier.

```rust,noplaypen
{{#include ../../../sessions/examples/sessions.rs}}
```

