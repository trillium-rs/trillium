use crate::http_types::{
    headers::{Header, Headers},
    Body, Method, StatusCode,
};
use std::convert::TryInto;
use std::fmt::{self, Debug, Formatter};
use trillium_http::ReceivedBody;

use crate::{BoxedTransport, Transport};

/**
# Trillium http connection

A Conn is the most important struct in trillium code. It represents
both the request and response of a http connection. trillium::Conn is
currently implemented as an abstraction on top of a
[`trillium_http::Conn`].

In particular, `trillium::Conn` boxes the transport using a
[`trillium::BoxedTransport`](crate::BoxedTransport) so that
application code can be written without transport generics. See
[`trillium::Transport`](crate::Transport) for further reading on this.


## method naming conventions

Additionally, `trillium::Conn` provides a notion of being `halted`
that is specifically designed for
[`trillium::Sequence`][crate::Sequence]s. This may eventually be
removed from Conn and instead be put into a SequenceConnExt for
consistency with other Handler types.

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
```

If you need to set a property on the conn without moving it,
`set_{attribute}` associated functions will be your huckleberry, as is
conventional in other rust projects.

## State

Every trillium Conn contains a state type which is a set that contains
at most one element for each type. It is highly recommended that you
not insert any type into the state set that you have not
authored. Library authors, this means that you should always offer a
ConnExt trait that provides an interface for setting and getting
state. State is also the primary way that handlers attach data to a
conn as it passes through a [`trillium::Sequence`][crate::Sequence].

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
    attempts to retrieve a &T from the state typemap

    ```
    struct Hello;
    let mut test_conn = trillium_testing::build_conn("GET", "/", None);
    assert!(test_conn.state::<Hello>().is_none());
    test_conn.set_state(Hello);
    assert!(test_conn.state::<Hello>().is_some());
    ```
    */
    pub fn state<T: 'static>(&self) -> Option<&T> {
        self.inner.state().get()
    }

    /// attempts to retrieve a &mut T from the state typemap
    pub fn state_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.inner.state_mut().get_mut()
    }

    /// puts a new type into the state typemap. see [`Conn::state`]
    /// for an example. returns the previous instance of this type, if
    /// any
    pub fn set_state<T: Send + Sync + 'static>(&mut self, val: T) -> Option<T> {
        self.inner.state_mut().insert(val)
    }

    /// puts a new type into the state typemap and returns the
    /// conn. this is useful for fluent chaining
    pub fn with_state<T: Send + Sync + 'static>(mut self, val: T) -> Self {
        self.set_state(val);
        self
    }

    /// removes a type from the state typemap and returns it, if present
    pub fn take_state<T: Send + Sync + 'static>(&mut self) -> Option<T> {
        self.inner.state_mut().remove()
    }

    /**
    either returns the current &mut T from the state typemap, or
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
    # futures_lite::future::block_on(async {
    let mut conn = trillium_testing::build_conn("GET", "/", "request body");
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
    if there is a response body for this conn and it has a known
    fixed length, it is returned from this function

    ```
    let conn = trillium_testing::build_conn("GET", "/", ()).with_body("hello");
    assert_eq!(conn.response_len(), Some(5));

    ```
    */
    pub fn response_len(&self) -> Option<u64> {
        self.inner.response_body().and_then(|b| b.len())
    }

    /**
    returns the request method for this conn.
    ```
    let conn = trillium_testing::build_conn("GET", "/", ()).with_body("hello");
    assert_eq!(conn.method(), &trillium::http_types::Method::Get);
    ```

    */
    pub fn method(&self) -> &Method {
        self.inner.method()
    }

    /**
    returns the response status for this conn, if it has been set.
    ```
    let mut conn = trillium_testing::build_conn("GET", "/", ());
    assert!(conn.status().is_none());
    conn.set_status(200);
    assert_eq!(conn.status().unwrap(), &trillium::http_types::StatusCode::Ok);
    ```
     */
    pub fn status(&self) -> Option<&StatusCode> {
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
    let conn = trillium_testing::build_conn("GET", "/", ()).with_status(200);
    assert_eq!(conn.status().unwrap(), &trillium::http_types::StatusCode::Ok);
    assert_eq!(*conn.status().unwrap(), 200);
    assert!(!conn.is_halted());
    ```
     */

    pub fn with_status(mut self, status: impl TryInto<StatusCode>) -> Self {
        self.set_status(status);
        self
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
    let conn = trillium_testing::build_conn("GET", "/", ())
        .with_header(("content-type", "application/html"));
    ```
    */
    pub fn with_header(mut self, header: impl Header) -> Self {
        self.headers_mut().apply(header);
        self
    }

    /// returns the path for this request. note that this may not
    /// represent the entire http request path if running nested
    /// routers.
    pub fn path(&self) -> &str {
        self.path
            .last()
            .map(|p| &**p)
            .unwrap_or_else(|| self.inner.path())
    }

    /// for router implementations. pushes a route segment onto the path
    pub fn push_path(&mut self, path: String) {
        self.path.push(path);
    }

    /// for router implementations. removes a route segment onto the path
    pub fn pop_path(&mut self) {
        self.path.pop();
    }

    /**
    sets the `halted` attribute of this conn, preventing later
    processing in a given [`trillium::Sequence`](crate::Sequence). returns
    the conn for fluent chaining

    ```
    let conn = trillium_testing::build_conn("GET", "/", ()).halt();
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
    let mut conn = trillium_testing::build_conn("GET", "/", ());
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

    /**
    sets the response body from any `impl Into<Body>` and returns the
    conn for fluent chaining. note that this does not set the response
    status or halted. See [`Conn::ok`] for a function that does both
    of those.

    ```
    let conn = trillium_testing::build_conn("GET", "/", ()).with_body("hello");
    assert_eq!(conn.response_len(), Some(5));
    ```
    */

    pub fn with_body(mut self, body: impl Into<Body>) -> Self {
        self.inner.set_response_body(body);
        self
    }

    /**
    Conn::ok is a convenience function for the common pattern of
    setting a body and a 200 status in one call. It is exactly
    identical to `conn.with_status(200).with_body(body).halt()`
    ```
    let conn = trillium_testing::build_conn("GET", "/", ()).ok("hello");
    assert_eq!(conn.response_len(), Some(5));
    assert_eq!(*conn.status().unwrap(), 200);
    assert!(conn.is_halted());
    ```
     */
    pub fn ok(self, body: impl Into<Body>) -> Conn {
        self.with_status(200).with_body(body).halt()
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
}
