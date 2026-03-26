# Custom extractors

You can extract any type by implementing [`FromConn`](crate::FromConn)
(infallible) or [`TryFromConn`](crate::TryFromConn) (fallible).

## `FromConn` — infallible extraction

Implement [`FromConn`](crate::FromConn) when extraction either always
succeeds or should silently skip the handler on failure.

Return `Some(value)` to proceed, or `None` to skip the handler (the conn
passes through unmodified).

### Extracting from conn state

The most common pattern — pull a shared resource out of conn state:

```rust
use trillium::Conn;
use trillium_api::FromConn;

#[derive(Clone, Debug)]
struct Db(/* ... */);

impl FromConn for Db {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        conn.state().cloned()
    }
}
```

Note this uses [`Conn::state`](trillium::Conn::state) (borrow + clone)
rather than [`Conn::take_state`](trillium::Conn::take_state). This is
important when multiple extractors or multiple requests need to access
the same state. Compare with [`State<T>`](crate::State), which calls
`take_state`.

### Extracting from request headers

```rust
use trillium::Conn;
use trillium_api::FromConn;

#[derive(Debug, Clone)]
struct BearerToken(String);

impl FromConn for BearerToken {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        conn.request_headers()
            .get_str("authorization")?
            .strip_prefix("Bearer ")
            .map(|t| BearerToken(t.to_owned()))
    }
}
```

### Caching extracted values

If extraction is expensive (e.g., a database query), you can cache
the result in conn state so subsequent extractors find it immediately:

```rust
use trillium::Conn;
use trillium_api::FromConn;

#[derive(Debug, Clone)]
struct CurrentUser { name: String }

impl FromConn for CurrentUser {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        // Check cache first
        if let Some(user) = conn.state::<Self>() {
            return Some(user.clone());
        }

        // Expensive lookup
        let token = conn.request_headers().get_str("authorization")?;
        let user = CurrentUser { name: token.to_string() }; // imagine a db lookup

        // Cache for later extractors
        conn.insert_state(user.clone());
        Some(user)
    }
}
```

## `TryFromConn` — fallible extraction

Implement [`TryFromConn`](crate::TryFromConn) when extraction can fail
with an error that should be reported to the client.

The key requirement: `TryFromConn::Error` must implement `Handler`. When
extraction fails, the error value is *run as a handler* on the conn. This
is how trillium-api turns extraction failures into HTTP responses.

### Extracting from route parameters

```rust
use trillium::Conn;
use trillium::{Handler, Status};
use trillium_api::TryFromConn;
use trillium_router::RouterConnExt;

#[derive(Debug, Clone)]
struct TodoId(u64);

impl TryFromConn for TodoId {
    type Error = Status;

    async fn try_from_conn(conn: &mut Conn) -> Result<Self, Status> {
        conn.param("todo_id")
            .and_then(|p| p.parse().ok())
            .map(TodoId)
            .ok_or(Status::BadRequest)
    }
}

// Now use it:
use trillium_api::{api, Json};

async fn show_todo(_conn: &mut Conn, TodoId(id): TodoId) -> String {
    format!("Todo #{id}")
}
# use trillium_testing::TestServer;
# use trillium_router::router;
# trillium_testing::block_on(async {
#     let app = TestServer::new(router().get("/todos/:todo_id", api(show_todo))).await;
#     app.get("/todos/42").await.assert_ok().assert_body("Todo #42");
#     app.get("/todos/abc").await.assert_status(Status::BadRequest);
# });
```

Using [`Status`](trillium::Status) as the error type is the simplest
option — `Status` implements `Handler` by setting the status code on the
conn. For richer errors, see [`error_handling`](crate::error_handling).

### Loading a resource from the database

A common pattern combines route parameter parsing with a database lookup,
so the handler receives a fully loaded domain object:

```rust
use trillium::Conn;
use trillium::{Handler, Status};
use trillium_api::{TryFromConn, FromConn};
use trillium_router::RouterConnExt;

# #[derive(Debug, Clone)] struct Db;
# impl FromConn for Db { async fn from_conn(conn: &mut Conn) -> Option<Self> { conn.state().cloned() } }
# #[derive(Debug, Clone, serde::Serialize)] struct Todo { id: u64, title: String }
impl TryFromConn for Todo {
    type Error = Status;

    async fn try_from_conn(conn: &mut Conn) -> Result<Self, Status> {
        let db = Db::from_conn(conn).await.ok_or(Status::InternalServerError)?;
        let id: u64 = conn
            .param("todo_id")
            .and_then(|p| p.parse().ok())
            .ok_or(Status::BadRequest)?;
        // db.find_todo(id).await.ok_or(Status::NotFound)
        # Ok(Todo { id, title: "example".into() })
    }
}
```

### Combining with other extractors

Custom extractors compose naturally with tuple extraction:

```rust,ignore
async fn update(
    _conn: &mut Conn,
    (todo, Body(update), db): (Todo, Body<UpdateTodo>, Db),
) -> Result<Json<Todo>, AppError> {
    // `todo` loaded via TryFromConn, `update` deserialized from body, `db` from state
}
```

## Blanket impl: `FromConn` → `TryFromConn`

Every `FromConn` type automatically implements `TryFromConn` with
`Error = ()`. Since `()` is the no-op handler, a failed infallible
extraction silently passes the conn through without setting any status
or body.
