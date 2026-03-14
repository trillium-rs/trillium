# Return types — handlers all the way down

The return type of an [`api`](crate::api) handler is not a response body
or a status code — it's a [`Handler`](trillium::Handler). The returned
handler is then *run on the conn*, giving you the full power of
trillium's handler model in your return value.

This is the key insight of trillium-api: rather than inventing a new
response type, it reuses the composable `Handler` trait that you already
know from the rest of trillium.

## The simplest returns

Several common types already implement `Handler`:

```rust
use trillium_api::api;
use trillium::{Conn, Status};

/// `()` is the no-op handler — doesn't set status, body, or halt.
/// Useful when you've already modified the conn via `&mut Conn`.
async fn modify_conn(conn: &mut Conn, _: ()) {
    conn.set_status(200);
    conn.set_body("done");
}

/// `&'static str` halts the conn with 200 + that string as the body.
async fn string_body(_conn: &mut Conn, _: ()) -> &'static str {
    "hello"
}

/// `Status` sets the status code (but does not halt or set a body).
async fn no_content(_conn: &mut Conn, _: ()) -> Status {
    Status::NoContent
}

# use trillium_testing::prelude::*;
# assert_ok!(get("/").on(&api(modify_conn)), "done");
# assert_ok!(get("/").on(&api(string_body)), "hello");
# assert_status!(get("/").on(&api(no_content)), Status::NoContent);
```

## JSON responses

[`Json<T>`](crate::Json) and [`Body<T>`](crate::Body) implement
`Handler` for `T: Serialize`, serializing the value and setting the
appropriate content type.

```rust
use trillium_api::{api, Body, Json};
use trillium::Conn;
use serde::Serialize;

#[derive(Serialize)]
struct User { name: String }

/// Json always serializes as application/json
async fn as_json(_conn: &mut Conn, _: ()) -> Json<User> {
    Json(User { name: "alice".into() })
}

/// Body negotiates the content type based on the Accept header
async fn as_body(_conn: &mut Conn, _: ()) -> Body<User> {
    Body(User { name: "alice".into() })
}

# use trillium_testing::prelude::*;
# assert_ok!(get("/").on(&api(as_json)), r#"{"name":"alice"}"#, "content-type" => "application/json");
```

## Tuples of handlers

Handler tuples run left to right, stopping at the first handler that
halts. This lets you compose multiple response properties:

```rust
use trillium_api::{api, Json};
use trillium::{Conn, Status};
use serde::Serialize;

#[derive(Serialize)]
struct Item { id: u64 }

/// Sets status to 201, then serializes the JSON body (which halts).
async fn create(_conn: &mut Conn, _: ()) -> (Status, Json<Item>) {
    (Status::Created, Json(Item { id: 42 }))
}

# use trillium_testing::prelude::*;
# let mut response = get("/").on(&api(create));
# assert_status!(&response, Status::Created);
# assert_eq!(response.take_response_body_string().unwrap(), r#"{"id":42}"#);
```

You can also include [`Headers`](trillium::Headers) in the tuple to set
response headers, or any other `Handler`.

## `Option<H>` — conditional responses

`Option<impl Handler>` runs the inner handler if `Some`, or does nothing
if `None` (no-op, doesn't halt):

```rust
use trillium_api::{api, Json};
use trillium::{Conn, Status};

async fn maybe(_conn: &mut Conn, _: ()) -> Option<Json<&'static str>> {
    if true { Some(Json("found")) } else { None }
}
```

## `Result<T, E>` — fallible responses

When both `T` and `E` implement `Handler`, `Result<T, E>` is also a
handler — running `T` on `Ok` or `E` on `Err`:

```rust
use trillium_api::{api, Json};
use trillium::{Conn, Handler, Status};

#[derive(serde::Serialize)]
struct ErrorBody { message: String }

async fn might_fail(_conn: &mut Conn, _: ()) -> Result<Json<&'static str>, (Status, Json<ErrorBody>)> {
    if true {
        Ok(Json("success"))
    } else {
        Err((Status::InternalServerError, Json(ErrorBody { message: "boom".into() })))
    }
}
# use trillium_testing::prelude::*;
# assert_ok!(get("/").on(&api(might_fail)), r#""success""#);
```

For the common case of a custom error type, see
[`error_handling`](crate::error_handling).

## Using `&mut Conn` directly

The first parameter is always `&mut Conn`. You can use it to set
response properties directly, and return `()` (or any handler) to
finish:

```rust
use trillium_api::api;
use trillium::Conn;

async fn direct(conn: &mut Conn, _: ()) {
    conn.set_status(200);
    conn.insert_response_header("x-custom", "value");
    conn.set_body("done");
}
# use trillium_testing::prelude::*;
# assert_ok!(get("/").on(&api(direct)), "done", "x-custom" => "value");
```

This is useful when you need to modify the conn in ways that don't
map cleanly to a return value — setting headers, caching state for
later extractors, or conditionally modifying the response.

## Important: concrete return types

Return types must be *concrete* — `-> impl Handler` does **not** work
as a return type for api handler functions. This is because the type
must be known at compile time for the `ApiHandler` struct's type
parameters. Use concrete types instead:

```rust,ignore
// Won't compile:
async fn bad(_conn: &mut Conn, _: ()) -> impl Handler { Json("hi") }

// Do this instead:
async fn good(_conn: &mut Conn, _: ()) -> Json<&'static str> { Json("hi") }
```

When you need to return different types from different branches,
use `Result`, `Option`, or a custom enum that implements `Handler`.

## `ApiHandler` sets 200 automatically

If your returned handler sets a response body but no status code,
[`api`](crate::api) automatically sets `200 OK`. You only need to
set a status explicitly when you want something other than 200.
