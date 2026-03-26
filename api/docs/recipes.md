# Recipes

Patterns and ideas for common use cases.

## Middleware with `api()`

An api handler can act as middleware by returning a handler that either
halts the conn (blocking downstream handlers) or does nothing (letting
them proceed).

The key trick: extract with `Option<T>` (which always succeeds), then
decide whether to halt.

```rust
use trillium_api::{api, FromConn, Halt};
use trillium::{Conn, Handler, Status};

#[derive(Debug, Clone)]
struct User(String);

impl FromConn for User {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        conn.request_headers()
            .get_str("x-user")
            .map(|s| User(s.to_owned()))
    }
}

async fn require_user(
    _conn: &mut Conn,
    user: Option<User>,
) -> Option<(Status, Halt)> {
    if user.is_none() {
        Some((Status::Forbidden, Halt))
    } else {
        None   // no-op — next handler runs
    }
}

// Place before your router in the handler tuple:
# use trillium_testing::TestServer;
# trillium_testing::block_on(async {
let app = TestServer::new((
    api(require_user),
    "hello, authenticated user",
)).await;
# app.get("/").await.assert_status(Status::Forbidden);
# app.get("/").with_request_header("x-user", "alice").await.assert_ok().assert_body("hello, authenticated user");
# });
```

## Type aliases for complex extractors

When tuple extractors get long, a type alias keeps handler signatures
readable:

```rust,ignore
type CreateArgs = (State<Arc<Db>>, Body<NewItem>, State<AppConfig>);

async fn create(
    conn: &mut Conn,
    (State(db), Body(input), State(config)): CreateArgs,
) -> Result<(Status, Json<Item>), AppError> {
    // ...
}
```

## `Arc<T>` for shared state

When your shared state is expensive to clone, wrap it in `Arc`. The
trillium `State<T>` handler clones `T` into each conn, so using
`Arc<T>` means only the pointer is cloned:

```rust,ignore
use std::sync::Arc;
use trillium_api::State;

struct Db { /* connection pool, etc. */ }

// In app setup:
let app = (
    State(Arc::new(Db { /* ... */ })),
    router,
);

// In handlers:
async fn list(_conn: &mut Conn, State(db): State<Arc<Db>>) -> Json<Vec<Item>> {
    // db is Arc<Db> — cheap to clone, shared across requests
}
```

## `FromConn` for shared state (borrow, don't take)

[`State<T>`](crate::State) calls
[`take_state`](trillium::Conn::take_state), which *removes* the value.
If you need the state to remain available for other handlers or
extractors, implement `FromConn` with `conn.state().cloned()` instead:

```rust,ignore
impl FromConn for Db {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        conn.state().cloned() // borrows, doesn't remove
    }
}
```

This is especially important for state that's used in multiple
extractors within the same request (e.g., a database handle used by
both a `User` extractor and the route handler itself).

## Returning `(Status, Json<T>)` for create endpoints

REST APIs commonly return `201 Created` with a body. Since `Status`
doesn't halt, and `Json<T>` does, they compose naturally:

```rust,ignore
async fn create(
    _conn: &mut Conn,
    (db, Body(input)): (Db, Body<NewItem>),
) -> Result<(Status, Json<Item>), AppError> {
    let item = db.insert(input).await?;
    Ok((Status::Created, Json(item)))
}
```

## Domain objects as extractors

Rather than parsing route parameters in every handler, implement
`TryFromConn` on your domain type to load it once from the route
param + database:

```rust,ignore
impl TryFromConn for Todo {
    type Error = Status;

    async fn try_from_conn(conn: &mut Conn) -> Result<Self, Status> {
        let db = Db::from_conn(conn).await.ok_or(Status::InternalServerError)?;
        let id: u64 = conn.param("todo_id")
            .and_then(|p| p.parse().ok())
            .ok_or(Status::BadRequest)?;
        db.find_todo(id).await.ok_or(Status::NotFound)
    }
}

// Now handlers receive a loaded Todo directly:
async fn show(_conn: &mut Conn, todo: Todo) -> Json<Todo> {
    Json(todo)
}

async fn update(
    _conn: &mut Conn,
    (todo, Body(input)): (Todo, Body<UpdateTodo>),
) -> Result<Json<Todo>, AppError> {
    // ...
}
```

## Query string extraction

There's no built-in query string extractor, but `TryFromConn` makes
it straightforward:

```rust,ignore
#[derive(Deserialize)]
struct Pagination {
    page: Option<u64>,
    per_page: Option<u64>,
}

impl TryFromConn for Pagination {
    type Error = Status;

    async fn try_from_conn(conn: &mut Conn) -> Result<Self, Status> {
        serde_urlencoded::from_str(conn.querystring())
            .map_err(|_| Status::BadRequest)
    }
}
```

## `cancel_on_disconnect` for expensive operations

[`cancel_on_disconnect`](crate::cancel_on_disconnect) is like
[`api`](crate::api), but cancels the handler future if the client
disconnects. The handler function does *not* receive `&mut Conn` —
all request data must come through extractors:

```rust,ignore
use trillium_api::cancel_on_disconnect;

async fn expensive_report(
    (db, pagination): (Db, Pagination),
) -> Result<Json<Report>, AppError> {
    // If the client hangs up, this future is dropped
    db.generate_report(pagination).await
        .map(Json)
        .map_err(AppError::from)
}

router().get("/report", cancel_on_disconnect(expensive_report))
```
