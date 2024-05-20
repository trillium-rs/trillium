use std::{
    fmt::{self, Debug, Formatter},
    future::Future,
    net::IpAddr,
};
use trillium_http::{
    transport::{BoxedTransport, Transport},
    type_set::entry::Entry,
    Body, HeaderName, HeaderValues, Headers, Method, ReceivedBody, Status, TypeSet,
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
    conn.with_response_header("content-type", "text/plain")
        .with_state(MyState("hello"))
        .with_body("hey there")
        .with_status(418)
}

use trillium_testing::prelude::*;

assert_response!(
    get("/").on(&handler),
    Status::ImATeapot,
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
handler. State access should generally be implemented by libraries
using a private type and exposed with a `ConnExt` trait. See [library
patterns](https://trillium.rs/library_patterns.html#state) for more
elaboration and examples.

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
    `Conn::ok` is a convenience function for the common pattern of
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
    #[must_use]
    pub fn ok(self, body: impl Into<Body>) -> Self {
        self.with_status(200).with_body(body).halt()
    }

    /**
    returns the response status for this `Conn`, if it has been set.
    ```
    use trillium_testing::prelude::*;
    let mut conn = get("/").on(&());
    assert!(conn.status().is_none());
    conn.set_status(200);
    assert_eq!(conn.status().unwrap(), Status::Ok);
    ```
     */
    pub fn status(&self) -> Option<Status> {
        self.inner.status()
    }

    /// assigns a status to this response. see [`Conn::status`] for example usage
    pub fn set_status(&mut self, status: impl TryInto<Status>) {
        self.inner.set_status(status);
    }

    /**
    sets the response status for this `Conn` and returns it. note that
    this does not set the halted status.

    ```
    use trillium_testing::prelude::*;
    let conn = get("/").on(&|conn: Conn| async move {
        conn.with_status(418)
    });
    let status = conn.status().unwrap();
    assert_eq!(status, Status::ImATeapot);
    assert_eq!(status, 418);
    assert!(!conn.is_halted());
    ```
     */

    #[must_use]
    pub fn with_status(mut self, status: impl TryInto<Status>) -> Self {
        self.set_status(status);
        self
    }

    /**
    Sets the response body from any `impl Into<Body>` and returns the
    `Conn` for fluent chaining. Note that this does not set the response
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

    #[must_use]
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
    Removes the response body from the `Conn`

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
    Borrows the response body from the `Conn`

    ```
    use trillium_testing::prelude::*;
    let mut conn = get("/").on(&());

    conn.set_body("hello");
    let body = conn.response_body().unwrap();
    assert_eq!(body.len(), Some(5));
    assert!(body.is_static());
    assert_eq!(body.static_bytes(), Some(&b"hello"[..]));
    ```
    */
    pub fn response_body(&self) -> Option<&Body> {
        self.inner.response_body()
    }

    /**
    Attempts to retrieve a &T from the state set

    ```
    use trillium_testing::prelude::*;

    struct Hello;
    let mut conn = get("/").on(&());
    assert!(conn.state::<Hello>().is_none());
    conn.insert_state(Hello);
    assert!(conn.state::<Hello>().is_some());
    ```
    */
    pub fn state<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.inner.state().get()
    }

    /// Attempts to receive a &T from the shared state set
    ///
    /// Note that shared state may not currently be mutated after server start, so there is no
    /// `shared_state_mut` or `shared_state_entry`
    pub fn shared_state<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.inner.shared_state().and_then(TypeSet::get)
    }

    /// Attempts to retrieve a &mut T from the state set
    pub fn state_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.inner.state_mut().get_mut()
    }

    /// Inserts a new type into the state set. See [`Conn::state`]
    /// for an example.
    ///
    /// Returns the previously-set instance of this type, if
    /// any
    pub fn insert_state<T: Send + Sync + 'static>(&mut self, state: T) -> Option<T> {
        self.inner.state_mut().insert(state)
    }

    /// Puts a new type into the state set and returns the
    /// `Conn`. this is useful for fluent chaining
    #[must_use]
    pub fn with_state<T: Send + Sync + 'static>(mut self, state: T) -> Self {
        self.insert_state(state);
        self
    }

    /// Removes a type from the state set and returns it, if present
    pub fn take_state<T: Send + Sync + 'static>(&mut self) -> Option<T> {
        self.inner.state_mut().take()
    }

    /// Returns an [`Entry`] type that represents the presence or absence of a type in this state.
    ///
    /// Use this for chainable combinators like [`Entry::or_default`], [`Entry::or_insert`],
    /// [`Entry::or_insert_with`], and [`Entry::and_modify`] as well as matching on it as an enum.
    pub fn state_entry<T: Send + Sync + 'static>(&mut self) -> Entry<'_, T> {
        self.inner.state_mut().entry()
    }

    /**
    Returns a [`ReceivedBody`] that references this `Conn`. The `Conn`
    retains all data and holds the singular transport, but the
    [`ReceivedBody`] provides an interface to read body content.

    See also: [`Conn::request_body_string`] for a convenience function
    when the content is expected to be utf8.


    # Examples

    ```
    use trillium_testing::prelude::*;
    let mut conn = get("/").with_request_body("request body").on(&());

    # trillium_testing::block_on(async {
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

    Convenience function to read the content of a request body as a `String`.

    # Errors

    This will return an error variant if either there is an IO failure
    on the underlying transport or if the body content is not a utf8
    string.

    # Examples

    ```
    use trillium_testing::prelude::*;
    let mut conn = get("/").with_request_body("request body").on(&());

    # trillium_testing::block_on(async {
    assert_eq!(conn.request_body_string().await.unwrap(), "request body");
    # });
    ```
    */
    #[allow(clippy::missing_errors_doc)] // this is a false positive
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
        self.inner.response_body().and_then(Body::len)
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

    /// borrow the response headers
    pub fn response_headers(&self) -> &Headers {
        self.inner.response_headers()
    }

    /// mutably borrow the response headers
    pub fn response_headers_mut(&mut self) -> &mut Headers {
        self.inner.response_headers_mut()
    }

    /// borrow the request headers
    pub fn request_headers(&self) -> &Headers {
        self.inner.request_headers()
    }

    /// mutably borrow request headers
    pub fn request_headers_mut(&mut self) -> &mut Headers {
        self.inner.request_headers_mut()
    }

    /// Insert a header name and value/values into the response headers and return the conn.
    ///
    /// See also [`Headers::insert`] and [`Headers::append`]
    ///
    /// For a slight performance improvement, use a [`KnownHeaderName`](crate::KnownHeaderName) as
    /// the first argument instead of a str.
    #[must_use]
    pub fn with_response_header(
        mut self,
        header_name: impl Into<HeaderName<'static>>,
        header_value: impl Into<HeaderValues>,
    ) -> Self {
        self.insert_response_header(header_name, header_value);
        self
    }

    /// Insert a header name and value/values into the response headers.
    ///
    /// See also [`Headers::insert`] and [`Headers::append`]
    ///
    /// For a slight performance improvement, use a [`KnownHeaderName`](crate::KnownHeaderName).
    pub fn insert_response_header(
        &mut self,
        header_name: impl Into<HeaderName<'static>>,
        header_value: impl Into<HeaderValues>,
    ) {
        self.response_headers_mut()
            .insert(header_name, header_value);
    }

    /**
    returns the path for this request. note that this may not
    represent the entire http request path if running nested
    routers.
    */
    pub fn path(&self) -> &str {
        self.path.last().map_or_else(|| self.inner.path(), |p| &**p)
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


    # Parsing

    Trillium does not include a querystring parsing library, as there is no universal standard for
    querystring encodings of arrays, but several library options exist, inluding:

    * [`QueryStrong`](https://docs.rs/querystrong/) (by the author of trillium)
    * [`serde_qs`](https://docs.rs/serde_qs/)
    * [`querystring`](https://docs.rs/querystring/)
    * [`serde_querystring`](https://docs.rs/serde-querystring/latest/serde_querystring/)

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
    #[must_use]
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
    pub const fn is_halted(&self) -> bool {
        self.halted
    }

    /// predicate function to indicate whether the connection is
    /// secure. note that this does not necessarily indicate that the
    /// transport itself is secure, as it may indicate that
    /// `trillium_http` is behind a trusted reverse proxy that has
    /// terminated tls and provided appropriate headers to indicate
    /// this.
    pub fn is_secure(&self) -> bool {
        self.inner.is_secure()
    }

    /// The [`Instant`] that the first header bytes for this conn were
    /// received, before any processing or parsing has been performed.
    pub fn start_time(&self) -> std::time::Instant {
        self.inner.start_time()
    }

    /// returns an immutable reference to the inner
    /// [`trillium_http::Conn`]. please open an issue if you need to do
    /// this in application code.
    ///
    /// stability note: hopefully this can go away at some point, but
    /// for now is an escape hatch in case `trillium_http::Conn`
    /// presents interfaces that cannot be reached otherwise.
    pub const fn inner(&self) -> &trillium_http::Conn<BoxedTransport> {
        &self.inner
    }

    /// returns a mutable reference to the inner
    /// [`trillium_http::Conn`]. please open an issue if you need to
    /// do this in application code.
    ///
    /// stability note: hopefully this can go away at some point, but
    /// for now is an escape hatch in case `trillium_http::Conn`
    /// presents interfaces that cannot be reached otherwise.
    pub fn inner_mut(&mut self) -> &mut trillium_http::Conn<BoxedTransport> {
        &mut self.inner
    }

    /// transforms this `trillium::Conn` into a `trillium_http::Conn`
    /// with the specified transport type. Please note that this will
    /// panic if you attempt to downcast from trillium's boxed
    /// transport into the wrong transport type. Also note that this
    /// is a lossy conversion, dropping the halted state and any
    /// nested router path data.
    ///
    /// # Panics
    ///
    /// This will panic if you attempt to downcast to the wrong Transport type.
    pub fn into_inner<T: Transport>(self) -> trillium_http::Conn<T> {
        self.inner.map_transport(|t| {
            *t.downcast()
                .expect("attempted to downcast to the wrong transport type")
        })
    }

    /// retrieves the remote ip address for this conn, if available.
    pub fn peer_ip(&self) -> Option<IpAddr> {
        self.inner().peer_ip()
    }

    /// sets the remote ip address for this conn.
    pub fn set_peer_ip(&mut self, peer_ip: Option<IpAddr>) {
        self.inner_mut().set_peer_ip(peer_ip);
    }

    /// for router implementations. pushes a route segment onto the path
    pub fn push_path(&mut self, path: String) {
        self.path.push(path);
    }

    /// for router implementations. removes a route segment onto the path
    pub fn pop_path(&mut self) {
        self.path.pop();
    }

    /// Cancels and drops the future if reading from the transport results in an error or empty read
    ///
    /// If the client disconnects from the conn's transport, this function will return None. If the
    /// future completes without disconnection, this future will return Some containing the output
    /// of the future.
    ///
    /// The use of this method is not advised if your connected http client employs pipelining
    /// (rarely seen in the wild), as it will buffer an unbounded number of requests
    ///
    /// Note that the inner future cannot borrow conn, so you will need to clone or take any
    /// information needed to execute the future prior to executing this method.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use trillium::{Conn, Method};
    /// async fn something_slow_and_cancel_safe() -> String {
    ///     String::from("this was not actually slow")
    /// }
    /// async fn handler(mut conn: Conn) -> Conn {
    ///     match conn.cancel_on_disconnect(async {
    ///         something_slow_and_cancel_safe().await
    ///     }).await {
    ///        Some(returned_body) => conn.ok(returned_body),
    ///        None => conn
    ///     }
    /// }
    /// ```
    pub async fn cancel_on_disconnect<'a, Fut>(&'a mut self, fut: Fut) -> Option<Fut::Output>
    where
        Fut: Future + Send + 'a,
    {
        self.inner_mut().cancel_on_disconnect(fut).await
    }

    /// Check if the transport is connected by testing attempting to read from the transport
    ///
    /// # Example
    ///
    /// This is best to use at appropriate points in a long-running handler, like:
    ///
    /// ```rust
    /// # use trillium::{Conn, Method};
    /// # async fn something_slow_but_not_cancel_safe() {}
    /// async fn handler(mut conn: Conn) -> Conn {
    ///     for _ in 0..100 {
    ///         if conn.is_disconnected().await {
    ///             return conn;
    ///         }
    ///         something_slow_but_not_cancel_safe().await;
    ///     }
    ///    conn.ok("ok!")
    /// }
    /// ```
    pub async fn is_disconnected(&mut self) -> bool {
        self.inner_mut().is_disconnected().await
    }
}

impl AsMut<TypeSet> for Conn {
    fn as_mut(&mut self) -> &mut TypeSet {
        self.inner.state_mut()
    }
}

impl AsRef<TypeSet> for Conn {
    fn as_ref(&self) -> &TypeSet {
        self.inner.state()
    }
}
