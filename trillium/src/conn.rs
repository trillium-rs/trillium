use crate::http_types::{
    headers::{Header, Headers},
    Body, Method, StatusCode,
};
use std::convert::TryInto;
use std::fmt::{self, Debug, Formatter};
use trillium_http::{
    transport::{BoxedTransport, Transport},
    ReceivedBody,
};

/**
# A Trillium HTTP connection.

A Conn represents both the request and response of a http connection,
as well as any application state that is associated with that
connection.

## `with_{attribute}` naming convention

A convention that is used throughout trillium is that any interface
that is named `with_{attribute}` will take ownership of the conn, set
the attribute and return the conn, enabling chained calls like:

```
struct MyState(&'static str);
async fn handler(mut conn: trillium::Conn) -> trillium::Conn {
    conn.with_header(("content-type", "text/plain"))
        .with_state(MyState("hello"))
        .with_body("hey there")
        .with_status(418)
}

use trillium_testing::prelude::*;

assert_response!(
    get("/").on(&handler),
    StatusCode::ImATeapot,
    "hey there",
    "content-type" => "text/plain"
);
```

If you need to set a property on the conn without moving it,
`set_{attribute}` associated functions will be your huckleberry, as is
conventional in other rust projects.

## State

Every trillium Conn contains a state type which is a set that contains
at most one element for each type. State is the primary way that
handlers attach data to a conn as it passes through a tuple
handler. In general, state access should generally be implemented by
libraries using a private type and exposed with a ConnExt trait. See
[library patterns](https://trillium.rs/library_patterns.html#state)
for more elaboration and examples.

## In relation to [`trillium_http::Conn`]

`trillium::Conn` is currently implemented as an abstraction on top of a
[`trillium_http::Conn`]. In particular, `trillium::Conn` boxes the
transport using a [`BoxedTransport`](trillium_http::transport::BoxedTransport)
so that application code can be written without transport
generics. See [`Transport`](trillium_http::transport::Transport) for further
reading on this.

*/

pub struct Conn {
    inner: trillium_http::Conn<BoxedTransport>,
    halted: bool,
    path: Vec<String>,
}

impl Debug for Conn {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Conn")
            .field("inner", &self.inner)
            .field("halted", &self.halted)
            .field("path", &self.path)
            .finish()
    }
}

impl<T: Transport + 'static> From<trillium_http::Conn<T>> for Conn {
    fn from(inner: trillium_http::Conn<T>) -> Self {
        Self {
            inner: inner.map_transport(BoxedTransport::new),
            halted: false,
            path: vec![],
        }
    }
}

impl Conn {
    /**
    Conn::ok is a convenience function for the common pattern of
    setting a body and a 200 status in one call. It is exactly
    identical to `conn.with_status(200).with_body(body).halt()`
    ```
    use trillium::Conn;
    use trillium_testing::prelude::*;
    let mut conn = get("/").on(&|conn: Conn| async move { conn.ok("hello") });
    assert_body!(&mut conn, "hello");
    assert_status!(&conn, 200);
    assert!(conn.is_halted());
    ```
     */
    pub fn ok(self, body: impl Into<Body>) -> Conn {
        self.with_status(200).with_body(body).halt()
    }

    /**
    returns the response status for this conn, if it has been set.
    ```
    use trillium_testing::prelude::*;
    let mut conn = get("/").on(&());
    assert!(conn.status().is_none());
    conn.set_status(200);
    assert_eq!(conn.status().unwrap(), StatusCode::Ok);
    ```
     */
    pub fn status(&self) -> Option<StatusCode> {
        self.inner.status()
    }

    /// assigns a status to this response. see [`Conn::status`] for example usage
    pub fn set_status(&mut self, status: impl TryInto<StatusCode>) {
        self.inner.set_status(status);
    }

    /**
    sets the response status for this conn and returns it. note that
    this does not set the halted status.

    ```
    use trillium_testing::prelude::*;
    let conn = get("/").on(&|conn: Conn| async move {
        conn.with_status(418)
    });
    let status = conn.status().unwrap();
    assert_eq!(status, StatusCode::ImATeapot);
    assert_eq!(status, 418);
    assert!(!conn.is_halted());
    ```
     */

    pub fn with_status(mut self, status: impl TryInto<StatusCode>) -> Self {
        self.set_status(status);
        self
    }

    /**
    Sets the response body from any `impl Into<Body>` and returns the
    conn for fluent chaining. Note that this does not set the response
    status or halted. See [`Conn::ok`] for a function that does both
    of those.

    ```
    use trillium_testing::prelude::*;
    let conn = get("/").on(&|conn: Conn| async move {
        conn.with_body("hello")
    });
    assert_eq!(conn.response_len(), Some(5));
    ```
    */

    pub fn with_body(mut self, body: impl Into<Body>) -> Self {
        self.set_body(body);
        self
    }

    /**
    Sets the response body from any `impl Into<Body>`. Note that this does not set the response
    status or halted.

    ```
    use trillium_testing::prelude::*;
    let mut conn = get("/").on(&());
    conn.set_body("hello");
    assert_eq!(conn.response_len(), Some(5));
    ```
    */
    pub fn set_body(&mut self, body: impl Into<Body>) {
        self.inner.set_response_body(body);
    }

    /**
    Removes the response body from the conn

    ```
    use trillium_testing::prelude::*;
    let mut conn = get("/").on(&());

    conn.set_body("hello");
    let mut body = conn.take_response_body().unwrap();
    assert_eq!(body.len(), Some(5));
    assert_eq!(conn.response_len(), None);
    ```
    */
    pub fn take_response_body(&mut self) -> Option<Body> {
        self.inner.take_response_body()
    }

    /**
    Attempts to retrieve a &T from the state set

    ```
    use trillium_testing::prelude::*;

    struct Hello;
    let mut conn = get("/").on(&());
    assert!(conn.state::<Hello>().is_none());
    conn.set_state(Hello);
    assert!(conn.state::<Hello>().is_some());
    ```
    */
    pub fn state<T: 'static>(&self) -> Option<&T> {
        self.inner.state().get()
    }

    /// Attempts to retrieve a &mut T from the state set
    pub fn state_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.inner.state_mut().get_mut()
    }

    /// Puts a new type into the state set. see [`Conn::state`]
    /// for an example. returns the previous instance of this type, if
    /// any
    pub fn set_state<T: Send + Sync + 'static>(&mut self, val: T) -> Option<T> {
        self.inner.state_mut().insert(val)
    }

    /// Puts a new type into the state set and returns the
    /// conn. this is useful for fluent chaining
    pub fn with_state<T: Send + Sync + 'static>(mut self, val: T) -> Self {
        self.set_state(val);
        self
    }

    /// Removes a type from the state set and returns it, if present
    pub fn take_state<T: Send + Sync + 'static>(&mut self) -> Option<T> {
        self.inner.state_mut().remove()
    }

    /**
    Either returns the current &mut T from the state set, or
    inserts a new one with the provided default function and
    returns a mutable reference to it
    */
    pub fn mut_state_or_insert_with<T, F>(&mut self, default: F) -> &mut T
    where
        T: Send + Sync + 'static,
        F: FnOnce() -> T,
    {
        self.inner.state_mut().get_or_insert_with(default)
    }

    /**
    returns a [ReceivedBody] that references this conn. the conn
    retains all data and holds the singular transport, but the
    ReceivedBody provides an interface to read body content
    ```
    # trillium_testing::block_on(async {
    use trillium_testing::prelude::*;
    let mut conn = get("/").with_request_body("request body").on(&());

    let request_body = conn.request_body().await;
    assert_eq!(request_body.content_length(), Some(12));
    assert_eq!(request_body.read_string().await.unwrap(), "request body");
    # });
    ```
    */
    pub async fn request_body(&mut self) -> ReceivedBody<'_, BoxedTransport> {
        self.inner.request_body().await
    }

    /**
    Convenience function to read the content of a request body as a String.
    ```
    # trillium_testing::block_on(async {
    use trillium_testing::prelude::*;
    let mut conn = get("/").with_request_body("request body").on(&());

    assert_eq!(conn.request_body_string().await.unwrap(), "request body");
    # });
    ```
    */

    pub async fn request_body_string(&mut self) -> trillium_http::Result<String> {
        self.request_body().await.read_string().await
    }

    /**
    if there is a response body for this conn and it has a known
    fixed length, it is returned from this function

    ```
    use trillium_testing::prelude::*;
    let mut conn = get("/").on(&|conn: trillium::Conn| async move {
        conn.with_body("hello")
    });

    assert_eq!(conn.response_len(), Some(5));
    ```
    */
    pub fn response_len(&self) -> Option<u64> {
        self.inner.response_body().and_then(|b| b.len())
    }

    /**
    returns the request method for this conn.
    ```
    use trillium_testing::prelude::*;
    let mut conn = get("/").on(&());

    assert_eq!(conn.method(), Method::Get);
    ```

    */
    pub fn method(&self) -> Method {
        self.inner.method()
    }

    /// returns the request headers
    ///
    /// stability note: this may become `request_headers` at some point
    pub fn headers(&self) -> &Headers {
        self.inner.request_headers()
    }

    /// returns the mutable response headers
    ///
    /// stability note: this may become `response_headers` at some point
    pub fn headers_mut(&mut self) -> &mut Headers {
        self.inner.response_headers()
    }

    /**
    apply a [`Header`] to the response headers and return the conn

    stability note: If trillium drops the dependence on http-types,
    this likely willl become `conn.with_header(&str, &str)`

    ```
    use trillium_testing::prelude::*;
    let mut conn = get("/").on(&|conn: trillium::Conn| async move {
        conn.with_header(("content-type", "application/html"))
    });
    ```
    */
    pub fn with_header(mut self, header: impl Header) -> Self {
        self.headers_mut().apply(header);
        self
    }

    /**
    returns the path for this request. note that this may not
    represent the entire http request path if running nested
    routers.
    */
    pub fn path(&self) -> &str {
        self.path
            .last()
            .map(|p| &**p)
            .unwrap_or_else(|| self.inner.path())
    }

    /**
    returns query part of the request path

    ```
    use trillium_testing::prelude::*;
    let conn = get("/a/b?c&d=e").on(&());
    assert_eq!(conn.querystring(), "c&d=e");

    let conn = get("/a/b").on(&());
    assert_eq!(conn.querystring(), "");
    ```
    */
    pub fn querystring(&self) -> &str {
        self.inner.querystring()
    }

    /**
    sets the `halted` attribute of this conn, preventing later
    processing in a given tuple handler. returns
    the conn for fluent chaining

    ```
    use trillium_testing::prelude::*;
    let mut conn = get("/").on(&|conn: trillium::Conn| async move {
        conn.halt()
    });

    assert!(conn.is_halted());
    ```
    */
    pub fn halt(mut self) -> Self {
        self.set_halted(true);
        self
    }

    /**
    sets the `halted` attribute of this conn. see [`Conn::halt`].

    ```
    use trillium_testing::prelude::*;
    let mut conn = get("/").on(&());
    assert!(!conn.is_halted());
    conn.set_halted(true);
    assert!(conn.is_halted());
    ```
    */
    pub fn set_halted(&mut self, halted: bool) {
        self.halted = halted;
    }

    /// retrieves the halted state of this conn.  see [`Conn::halt`].
    pub fn is_halted(&self) -> bool {
        self.halted
    }

    /// predicate function to indicate whether the connection is
    /// secure. note that this does not necessarily indicate that the
    /// transport itself is secure, as it may indicate that
    /// trillium_http is behind a trusted reverse proxy that has
    /// terminated tls and provided appropriate headers to indicate
    /// this.
    pub fn is_secure(&self) -> bool {
        self.inner.is_secure()
    }

    /// returns an immutable reference to the inner
    /// [`trillium_http::Conn`]. please open an issue if you need to do
    /// this in application code.
    ///
    /// stability note: hopefully this can go away at some point,
    /// but for now is an escape hatch in case trillium_http::Conn
    /// presents interface that cannot be reached otherwise.
    pub fn inner(&self) -> &trillium_http::Conn<BoxedTransport> {
        &self.inner
    }

    /// returns a mutable reference to the inner
    /// [`trillium_http::Conn`]. please open an issue if you need to
    /// do this in application code.
    ///
    /// stability note: hopefully this can go away at some point,
    /// but for now is an escape hatch in case trillium_http::Conn
    /// presents interface that cannot be reached otherwise.
    pub fn inner_mut(&mut self) -> &mut trillium_http::Conn<BoxedTransport> {
        &mut self.inner
    }

    /// transforms this trillium::Conn into a `trillium_http::Conn`
    /// with the specified transport type. Please note that this will
    /// panic if you attempt to downcast from trillium's boxed
    /// transport into the wrong transport type. Also note that this
    /// is a lossy conversion, dropping the halted state and any
    /// nested router path data.
    pub fn into_inner<T: Transport>(self) -> trillium_http::Conn<T> {
        self.inner.map_transport(|t| {
            *t.downcast()
                .expect("attempted to downcast to the wrong transport type")
        })
    }

    /// for router implementations. pushes a route segment onto the path
    pub fn push_path(&mut self, path: String) {
        self.path.push(path);
    }

    /// for router implementations. removes a route segment onto the path
    pub fn pop_path(&mut self) {
        self.path.pop();
    }
}
