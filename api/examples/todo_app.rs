// # trillium-api example: a simple todo list API
//
// This example demonstrates the key patterns of trillium-api:
//
// - **Extractors**: pulling typed data out of requests (`Body`, `Json`, `State`, custom `FromConn`)
// - **Handler return types**: api handlers return `impl Handler`, not raw responses
// - **Error handling**: using `Result<impl Handler, Error>` and custom error types
// - **Tuple extraction**: combining multiple extractors as a tuple
// - **Middleware via `api()`**: using `FromConn` to gate access (e.g., authentication)

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};
use trillium::{Conn, Handler, Status};
use trillium_api::{ApiConnExt, Body, FromConn, Json, State, TryFromConn, Value, api};
use trillium_router::{RouterConnExt, router};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Todo {
    id: u64,
    title: String,
    completed: bool,
    owner: String,
}

#[derive(Debug, Deserialize)]
struct NewTodo {
    title: String,
    completed: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct UpdateTodo {
    title: Option<String>,
    completed: Option<bool>,
}

// ---------------------------------------------------------------------------
// "Database" — shared state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct Db {
    todos: Arc<RwLock<HashMap<u64, Todo>>>,
    next_id: Arc<AtomicU64>,
}

/// Implement `Handler` so `Db` can be placed in a handler tuple to inject
/// itself into every conn's state
impl Handler for Db {
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_state(self.clone())
    }
}

/// Implement `FromConn` so `Db` can be extracted directly in api handlers.
/// This clones the shared handle out of conn state
impl FromConn for Db {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        conn.state().cloned()
    }
}

// ---------------------------------------------------------------------------
// Authentication — a simple extractor that gates access
// ---------------------------------------------------------------------------

/// A simple "user" extracted from a request header. If the header is missing,
/// `from_conn` returns `None`, which halts the conn (the api handler is never
/// called and the default 404 is returned).
#[derive(Debug, Clone)]
struct User(String);

impl FromConn for User {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        conn.request_headers()
            .get_str("x-user")
            .map(|s| User(s.to_owned()))
    }
}

// ---------------------------------------------------------------------------
// Custom `TryFromConn` — extracting a Todo by route param
// ---------------------------------------------------------------------------

/// Extract a `Todo` from the route parameter `:todo_id` and the database.
///
/// If extraction fails, the `Error` type (which must implement `Handler`) is
/// run on the conn instead of the api handler body.
impl TryFromConn for Todo {
    type Error = Status;

    async fn try_from_conn(conn: &mut Conn) -> Result<Self, Status> {
        let db = Db::from_conn(conn)
            .await
            .ok_or(Status::InternalServerError)?;
        let id: u64 = conn
            .param("todo_id")
            .and_then(|p| p.parse().ok())
            .ok_or(Status::BadRequest)?;
        let todos = db.todos.read().unwrap();
        todos.get(&id).cloned().ok_or(Status::NotFound)
    }
}

// ---------------------------------------------------------------------------
// Authentication middleware via `api()`
// ---------------------------------------------------------------------------

/// When used as `api(require_user)` in a handler tuple before the router,
/// this acts as middleware. It uses `Option<User>` as the extractor (which
/// always succeeds — `Option<T: FromConn>` extracts to `Some(t)` or `None`).
///
/// If the user is missing, it returns `Some((Status::Forbidden, Halt))`, which
/// halts the conn, preventing downstream handlers from running.
///
/// If the user is present, it returns `None` — the no-op handler — so the
/// next handler in the tuple proceeds normally.
async fn require_user(
    _conn: &mut Conn,
    user: Option<User>,
) -> Option<(Status, trillium_api::Halt)> {
    if user.is_none() {
        Some((Status::Forbidden, trillium_api::Halt))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Custom error type
// ---------------------------------------------------------------------------

/// Application error type. Implements `Handler` so it can be used as the `Err`
/// variant of `Result<impl Handler, AppError>`.
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "error")]
enum AppError {
    #[serde(rename = "not_found")]
    NotFound { message: String },
    #[serde(rename = "bad_request")]
    BadRequest { message: String },
}

impl Handler for AppError {
    async fn run(&self, conn: Conn) -> Conn {
        let status = match self {
            AppError::NotFound { .. } => Status::NotFound,
            AppError::BadRequest { .. } => Status::BadRequest,
        };
        conn.with_json(self).with_status(status).halt()
    }
}

// ---------------------------------------------------------------------------
// API handler functions
// ---------------------------------------------------------------------------

/// `GET /` — a simple health check.
///
/// Demonstrates the simplest possible api handler — no extraction needed
/// (`()` is a no-op extractor), returns a static string (which implements
/// `Handler` by halting with 200 + that body).
async fn health(_conn: &mut Conn, _: ()) -> &'static str {
    "ok"
}

/// `GET /todos` — list all todos.
///
/// Extracts `Db` via `FromConn`. Returns `Json<Vec<Todo>>`, which serializes
/// the list and sets `Content-Type: application/json`.
async fn list_todos(_conn: &mut Conn, db: Db) -> Json<Vec<Todo>> {
    let todos = db.todos.read().unwrap();
    Json(todos.values().cloned().collect())
}

/// `POST /todos` — create a new todo.
///
/// Tuple extraction: `(User, Body<NewTodo>, Db)` extracts the authenticated
/// user, deserializes the request body (with content-type negotiation), and
/// gets the database handle.
///
/// Returns `(Status, Json<Todo>)` — a tuple of handlers. The `Status` handler
/// sets 201 Created, and `Json` serializes the response.
async fn create_todo(
    _conn: &mut Conn,
    (User(owner), Body(new_todo), db): (User, Body<NewTodo>, Db),
) -> (Status, Json<Todo>) {
    let id = db.next_id.fetch_add(1, Ordering::Relaxed);
    let todo = Todo {
        id,
        title: new_todo.title,
        completed: new_todo.completed.unwrap_or(false),
        owner,
    };
    db.todos.write().unwrap().insert(id, todo.clone());
    (Status::Created, Json(todo))
}

/// `GET /todos/:todo_id` — show a single todo.
///
/// `Todo` is extracted via its `TryFromConn` impl, which looks up the route
/// param in the database. If the todo doesn't exist, the handler is never
/// called — instead, `Status::NotFound` (the `TryFromConn::Error`) is run.
async fn show_todo(_conn: &mut Conn, todo: Todo) -> Json<Todo> {
    Json(todo)
}

/// `PATCH /todos/:todo_id` — update a todo.
///
/// Returns `Result<impl Handler, AppError>`. Since both `Json<Todo>` and
/// `AppError` implement `Handler`, the `Result` itself is a `Handler` —
/// on `Ok`, the todo is serialized; on `Err`, the error handler runs.
async fn update_todo(
    _conn: &mut Conn,
    (mut todo, Body(update), db): (Todo, Body<UpdateTodo>, Db),
) -> Result<Json<Todo>, AppError> {
    if let Some(title) = update.title {
        if title.is_empty() {
            return Err(AppError::BadRequest {
                message: "title cannot be empty".into(),
            });
        }
        todo.title = title;
    }
    if let Some(completed) = update.completed {
        todo.completed = completed;
    }
    db.todos.write().unwrap().insert(todo.id, todo.clone());
    Ok(Json(todo))
}

/// `DELETE /todos/:todo_id` — delete a todo.
///
/// Returns `Status::NoContent` directly. `Status` implements `Handler`,
/// so this sets the status code with no response body.
async fn delete_todo(_conn: &mut Conn, (todo, db): (Todo, Db)) -> Status {
    db.todos.write().unwrap().remove(&todo.id);
    Status::NoContent
}

/// `GET /todos/search?q=...` — search todos by title.
///
/// Demonstrates using `Result<impl Handler, AppError>` with our custom error
/// type, including the `NotFound` variant.
async fn search_todos(conn: &mut Conn, db: Db) -> Result<Json<Vec<Todo>>, AppError> {
    let query = conn
        .querystring()
        .split('&')
        .find_map(|pair| pair.strip_prefix("q="))
        .ok_or_else(|| AppError::BadRequest {
            message: "missing query parameter `q`".into(),
        })?;
    let todos = db.todos.read().unwrap();
    let matches: Vec<Todo> = todos
        .values()
        .filter(|t| t.title.contains(query))
        .cloned()
        .collect();
    if matches.is_empty() {
        Err(AppError::NotFound {
            message: format!("no todos matching \"{query}\""),
        })
    } else {
        Ok(Json(matches))
    }
}

/// `GET /me` — return info about the current user.
///
/// Demonstrates extracting `User` (which comes from a header) and
/// `State<String>` (which comes from shared handler state set at startup).
async fn me(_conn: &mut Conn, (User(name), State(app_name)): (User, State<String>)) -> Json<Value> {
    Json(trillium_api::json!({
        "user": name,
        "app": app_name,
    }))
}

// ---------------------------------------------------------------------------
// Custom error handler
// ---------------------------------------------------------------------------

/// Intercepts application errors in `before_send` and formats them.
///
/// Placed at the *beginning* of the handler tuple so its `before_send` runs
/// *last* (before_send is called in reverse tuple order). This catches any
/// `AppError` that a handler placed into conn state.
///
/// Note: `trillium_api::Error` (e.g., JSON parse failures) is handled by
/// its own `before_send` inside the `ApiHandler` — to customize *that*
/// formatting, you'd need a different error type as your `TryFromConn::Error`.
#[derive(Copy, Clone, Debug)]
struct CustomErrorHandler;

impl Handler for CustomErrorHandler {
    async fn run(&self, conn: Conn) -> Conn {
        conn
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        if let Some(error) = conn.take_state::<AppError>() {
            let status = match &error {
                AppError::NotFound { .. } => Status::NotFound,
                AppError::BadRequest { .. } => Status::BadRequest,
            };
            conn.with_json(&error).with_status(status)
        } else {
            conn
        }
    }
}

// ---------------------------------------------------------------------------
// Application setup
// ---------------------------------------------------------------------------

fn app() -> impl Handler {
    let db = Db::default();

    (
        // Shared state: `Db` is a Handler that clones itself into each
        // conn's state, making it available to `FromConn` extractors.
        db,
        // Shared state: a plain string via trillium's built-in `State`.
        trillium::State::new("Todo App".to_string()),
        // Custom error handler — placed early in the tuple so its
        // `before_send` runs last, after inner handlers have processed.
        CustomErrorHandler,
        // Authentication middleware — gates all routes. If the `x-user`
        // header is missing, responds with 403 and halts.
        api(require_user),
        router()
            // `api(handler_fn)` wraps each handler function, providing
            // extraction and handler-return-type support.
            .get("/", api(health))
            .get("/me", api(me))
            .get("/todos", api(list_todos))
            .get("/todos/search", api(search_todos))
            .post("/todos", api(create_todo))
            .get("/todos/:todo_id", api(show_todo))
            .patch("/todos/:todo_id", api(update_todo))
            .delete("/todos/:todo_id", api(delete_todo)),
    )
}

fn main() {
    env_logger::init();
    trillium_smol::run(app());
}

#[cfg(test)]
mod tests {
    use super::*;
    use trillium_testing::prelude::*;

    #[test]
    fn test_list_empty() {
        assert_ok!(
            get("/todos")
                .with_request_header("x-user", "alice")
                .on(&app()),
            "[]"
        );
    }

    #[test]
    fn test_create_and_show() {
        let app = app();
        let mut response = post("/todos")
            .with_request_header("x-user", "alice")
            .with_request_header("content-type", "application/json")
            .with_request_body(r#"{"title": "buy milk"}"#)
            .on(&app);
        assert_status!(&response, Status::Created);
        let body = response.take_response_body_string().unwrap();
        assert!(body.contains("buy milk"));
        assert!(body.contains("alice"));

        // Show the created todo
        let mut show = get("/todos/0")
            .with_request_header("x-user", "alice")
            .on(&app);
        assert_status!(&show, Status::Ok);
        let show_body = show.take_response_body_string().unwrap();
        assert!(show_body.contains("buy milk"));
    }

    #[test]
    fn test_update() {
        let app = app();
        post("/todos")
            .with_request_header("x-user", "alice")
            .with_request_header("content-type", "application/json")
            .with_request_body(r#"{"title": "buy milk"}"#)
            .on(&app);

        let mut response = patch("/todos/0")
            .with_request_header("x-user", "alice")
            .with_request_header("content-type", "application/json")
            .with_request_body(r#"{"completed": true}"#)
            .on(&app);
        assert_status!(&response, Status::Ok);
        let body = response.take_response_body_string().unwrap();
        assert!(body.contains("true"));
    }

    #[test]
    fn test_update_empty_title_returns_error() {
        let app = app();
        post("/todos")
            .with_request_header("x-user", "alice")
            .with_request_header("content-type", "application/json")
            .with_request_body(r#"{"title": "buy milk"}"#)
            .on(&app);

        assert_status!(
            patch("/todos/0")
                .with_request_header("x-user", "alice")
                .with_request_header("content-type", "application/json")
                .with_request_body(r#"{"title": ""}"#)
                .on(&app),
            Status::BadRequest
        );
    }

    #[test]
    fn test_delete() {
        let app = app();
        post("/todos")
            .with_request_header("x-user", "alice")
            .with_request_header("content-type", "application/json")
            .with_request_body(r#"{"title": "buy milk"}"#)
            .on(&app);

        assert_status!(
            delete("/todos/0")
                .with_request_header("x-user", "alice")
                .on(&app),
            Status::NoContent
        );

        // Should be gone
        assert_status!(
            get("/todos/0")
                .with_request_header("x-user", "alice")
                .on(&app),
            Status::NotFound
        );
    }

    #[test]
    fn test_not_found() {
        assert_status!(
            get("/todos/999")
                .with_request_header("x-user", "alice")
                .on(&app()),
            Status::NotFound
        );
    }

    #[test]
    fn test_missing_auth_returns_forbidden() {
        // Without the x-user header, the `require_user` middleware
        // returns `Some((Status::Forbidden, Halt))`, which halts the conn
        // with a 403 before any route handler runs.
        assert_status!(get("/todos").on(&app()), Status::Forbidden);
    }

    #[test]
    fn test_bad_json() {
        let mut response = post("/todos")
            .with_request_header("x-user", "alice")
            .with_request_header("content-type", "application/json")
            .with_request_body("not json")
            .on(&app());
        assert_status!(&response, Status::UnprocessableEntity);
        let body = response.take_response_body_string().unwrap();
        // trillium_api::Error's default before_send formats as JSON
        assert!(body.contains("parse_error"), "got: {body}");
    }

    #[test]
    fn test_me() {
        let mut response = get("/me").with_request_header("x-user", "alice").on(&app());
        assert_status!(&response, Status::Ok);
        let body = response.take_response_body_string().unwrap();
        assert!(body.contains("alice"));
        assert!(body.contains("Todo App"));
    }
}
