use crate::{Runtime, RuntimeTrait, ServerConnector, TestTransport, runtime};
use async_channel::Sender;
use std::{
    any::{Any, type_name},
    fmt::{self, Debug, Formatter},
    future::{Future, IntoFuture},
    net::IpAddr,
    pin::Pin,
    str,
    sync::Arc,
};
use trillium::{Handler, Info, KnownHeaderName};
use trillium_client::{Client, IntoUrl};
use trillium_http::{HeaderName, HeaderValues, Headers, HttpContext, Method, Status};
#[allow(clippy::test_attr_in_doctest, reason = "demonstrating test usage")]
/// A testing interface that wraps a trillium handler, providing a high-level API for making
/// requests and asserting on responses.
///
/// This runs a full request-response cycle against an in-memory virtual transport using
/// [`trillium-client`](https://docs.rs/trillium-client). No ports are bound and the tests are fully
/// parallelizable.
///
/// A fluent set of assertions are provided that chain off of a borrow.
///
/// ```
/// use test_harness::test;
/// use trillium::{Conn, Status};
/// use trillium_testing::{TestResult, TestServer, harness};
///
/// #[test(harness)]
/// async fn basic_test() {
///     let app = TestServer::new(|conn: Conn| async move { conn.ok("hello") }).await;
///
///     app.get("/").await.assert_ok().assert_body("hello");
///
///     // or if you prefer:
///
///     let conn = app.post("/").with_body("body").await;
///     conn.assert_ok();
///     conn.assert_body("hello");
/// }
///
/// // also an option, but not preferred:
///
/// #[test]
/// fn sync_test() {
///     let app = TestServer::new_blocking(|conn: Conn| async move { conn.ok("hello") });
///
///     app.get("/").block().assert_ok().assert_body("hello");
///
///     let conn = app.post("/").with_body("body").block();
///     conn.assert_ok();
///     conn.assert_body("hello");
/// }
/// ```
#[derive(Clone, Debug)]
pub struct TestServer<H> {
    client: Client,
    peer_ip_sender: Sender<IpAddr>,
    connector: ServerConnector<H>,
}

impl<H: Handler> TestServer<H> {
    /// Creates a new [`TestServer`].
    ///
    /// Note that this is **async** because it initializes the handler with [`Handler::init`].
    pub async fn new(handler: H) -> Self {
        Self::new_with_runtime(handler, runtime()).await
    }

    async fn new_with_runtime(mut handler: H, rt: impl RuntimeTrait) -> Self {
        let url = "http://trillium.test".into_url(None).unwrap();
        let mut info = Info::from(HttpContext::default());
        info.insert_shared_state(rt.clone());
        info.insert_shared_state(Runtime::new(rt.clone()));
        info.insert_shared_state(url.clone());
        handler.init(&mut info).await;
        let context: Arc<HttpContext> = Arc::new(info.into());
        let mut connector = ServerConnector::new(handler)
            .with_context(context.clone())
            .with_runtime(rt);
        let (peer_ip_sender, receive) = async_channel::unbounded();
        connector.server_peer_ips_receiver = Some(receive);
        let client = Client::new(connector.clone()).with_base(url);

        Self {
            client,
            peer_ip_sender,
            connector,
        }
    }

    /// construct a new TestServer and block on initialization
    pub fn new_blocking(handler: H) -> Self {
        // Create the runtime before block_on so it is stored as an owned (not borrowed) runtime
        // in the connector. If we used crate::block_on here, runtime() inside new_with_runtime
        // would detect the current tokio handle and store AlreadyRunning — pointing at the
        // temporary block_on runtime — which is shut down before block() is ever called.
        let rt = runtime();
        rt.clone().block_on(Self::new_with_runtime(handler, rt))
    }

    /// Build a new [`ConnTest`]
    pub fn build<M: TryInto<Method>>(&self, method: M, path: &str) -> ConnTest
    where
        M::Error: Debug,
    {
        ConnTest {
            inner: self.client.build_conn(method, path),
            body: None,
            peer_ip_sender: self.peer_ip_sender.clone(),
            peer_ip: None,
            runtime: self.connector.runtime().clone(),
        }
    }

    /// borrow from shared state configured by the handler
    pub fn shared_state<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.connector.context().shared_state().get()
    }

    /// assert that a given type is in shared state
    #[track_caller]
    pub fn assert_shared_state<T>(&self, expected: T) -> &Self
    where
        T: Send + Sync + Debug + PartialEq + 'static,
    {
        match self.shared_state::<T>() {
            Some(actual) => assert_eq!(*actual, expected),
            None => panic!(
                "expected handler state of type {}, but none was found",
                type_name::<T>()
            ),
        };
        self
    }

    /// assert that a given type is in shared and make further assertions on it
    pub fn assert_shared_state_with<T, F>(&self, f: F) -> &Self
    where
        T: Send + Sync + 'static,
        F: FnOnce(&T),
    {
        match self.shared_state::<T>() {
            Some(state) => f(state),
            None => panic!(
                "expected handler state of type {}, but none was found",
                type_name::<T>()
            ),
        };
        self
    }

    /// Borrow the handler
    pub fn handler(&self) -> &H {
        self.connector.handler()
    }

    /// Add a default host/authority for this virtual server (eg pretend this server is running at
    /// `example.com` with `.with_host("example.com")`
    pub fn with_host(mut self, host: &str) -> Self {
        self.set_host(host);
        self
    }

    /// Set the default host/authority for this virtual server (eg pretend this server is running at
    /// `example.com` with `.set_host("example.com")`
    pub fn set_host(&mut self, host: &str) -> &mut Self {
        let _ = self.client.base_mut().unwrap().set_host(Some(host));
        self
    }

    /// Set the url for this virtual server (eg pretend this server is running at
    /// `https://example.com` with `.with_base("https://example.com")`
    pub fn with_base(mut self, base: impl IntoUrl) -> Self {
        self.set_base(base);
        self
    }

    /// Set the url for this virtual server (eg pretend this server is running at
    /// `https://example.com` with `.set_base("https://example.com")`
    pub fn set_base(&mut self, base: impl IntoUrl) -> &mut Self {
        self.client
            .set_base(base)
            .expect("unable to build a base url");
        self
    }

    /// Builds a GET [`ConnTest`] for the given path.
    pub fn get(&self, path: &str) -> ConnTest {
        self.build(Method::Get, path)
    }

    /// Builds a POST [`ConnTest`] for the given path.
    pub fn post(&self, path: &str) -> ConnTest {
        self.build(Method::Post, path)
    }

    /// Builds a PUT [`ConnTest`] for the given path.
    pub fn put(&self, path: &str) -> ConnTest {
        self.build(Method::Put, path)
    }

    /// Builds a DELETE [`ConnTest`] for the given path.
    pub fn delete(&self, path: &str) -> ConnTest {
        self.build(Method::Delete, path)
    }

    /// Builds a PATCH [`ConnTest`] for the given path.
    pub fn patch(&self, path: &str) -> ConnTest {
        self.build(Method::Patch, path)
    }
}

/// Represents both an outbound HTTP request being built and the received HTTP response.
///
/// Before `.await`, use the request-building methods to configure the request.
/// After `.await`, use the accessor and assertion methods to inspect the response.
///
/// The response body is read eagerly on `.await`, so all accessors are synchronous.
pub struct ConnTest {
    inner: trillium_client::Conn,
    body: Option<Vec<u8>>,
    peer_ip_sender: Sender<IpAddr>,
    peer_ip: Option<IpAddr>,
    runtime: Runtime,
}

impl Debug for ConnTest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnTest")
            .field("inner", &self.inner)
            .field("body", &self.body.as_deref().map(String::from_utf8_lossy))
            .finish()
    }
}

/// Request-building methods (use before `.await`)
impl ConnTest {
    /// Inserts a request header, replacing any existing value for that header name.
    pub fn with_request_header(
        mut self,
        name: impl Into<HeaderName<'static>>,
        value: impl Into<HeaderValues>,
    ) -> Self {
        self.inner.request_headers_mut().insert(name, value);
        self
    }

    /// Extends the request headers from an iterable of `(name, value)` pairs.
    pub fn with_request_headers<HN, HV, I>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = (HN, HV)> + Send,
        HN: Into<HeaderName<'static>>,
        HV: Into<HeaderValues>,
    {
        self.inner.request_headers_mut().extend(headers);
        self
    }

    /// Removes a request header if present.
    pub fn without_request_header(mut self, name: impl Into<HeaderName<'static>>) -> Self {
        self.inner.request_headers_mut().remove(name);
        self
    }

    /// Sets the request body.
    pub fn with_body(mut self, body: impl Into<trillium_http::Body>) -> Self {
        self.inner.set_request_body(body);
        self
    }

    /// Sets the request body to the given serializable, as well as setting content-type:
    /// application/json if not already set
    #[cfg(any(feature = "sonic-rs", feature = "serde_json"))]
    pub fn with_json_body(mut self, body: &impl serde::Serialize) -> Self {
        self.inner
            .request_headers_mut()
            .try_insert(KnownHeaderName::ContentType, "application/json");

        self.with_body(crate::to_json_string(body).unwrap())
    }

    /// Sets a test-double ip that represents the *client's* ip, available to the server as peer ip.
    pub fn with_peer_ip(mut self, peer_ip: impl Into<IpAddr>) -> Self {
        self.peer_ip = Some(peer_ip.into());
        self
    }

    /// Perform a blocking request
    pub fn block(self) -> Self {
        self.runtime.clone().block_on(self.into_future())
    }
}

/// Response accessors and assertions (use after `.await`)
impl ConnTest {
    /// Returns handler state of type `T` set on the conn during the request, if any.
    pub fn state<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.inner.state::<T>()
    }

    /// Asserts that handler state of type `T` was set and equals `expected`.
    #[track_caller]
    pub fn assert_state<T>(&self, expected: T) -> &Self
    where
        T: PartialEq + Debug + Send + Sync + 'static,
    {
        match self.state::<T>() {
            Some(actual) => assert_eq!(*actual, expected),
            None => panic!(
                "expected handler state of type {}, but none was found",
                type_name::<T>()
            ),
        }
        self
    }

    /// Asserts that no handler state of type `T` was set on the conn during the request.
    #[track_caller]
    pub fn assert_no_state<T>(&self) -> &Self
    where
        T: Debug + Send + Sync + 'static,
    {
        if let Some(value) = self.state::<T>() {
            panic!(
                "expected no handler state of type {}, but found {:?}",
                type_name::<T>(),
                value
            );
        }
        self
    }

    /// Returns the response status code.
    ///
    /// Panics if called before the request has been sent (i.e., before `.await`).
    pub fn status(&self) -> Status {
        self.inner
            .status()
            .expect("response not yet received — did you .await this ConnTest?")
    }

    /// Returns the response body as a `&str`.
    ///
    /// Panics if no body was received from the server, or if the body is not a valid utf-8 string.
    pub fn body(&self) -> &str {
        str::from_utf8(self.body_bytes()).expect("body was not utf-8")
    }

    /// Returns the response body as a `&str`.
    ///
    /// Panics if no body was received from the server
    pub fn body_bytes(&self) -> &[u8] {
        self.body.as_deref().expect("body was not set")
    }

    /// Returns the response headers.
    pub fn response_headers(&self) -> &Headers {
        self.inner.response_headers()
    }

    /// Returns the response headers.
    pub fn response_trailers(&self) -> Option<&Headers> {
        self.inner.response_trailers()
    }

    /// Returns the response headers.
    pub fn request_trailers(&self) -> Option<&Headers> {
        self.inner.request_trailers()
    }

    /// Returns the value of a response header by name, if present.
    pub fn header<'a>(&self, name: impl Into<HeaderName<'a>>) -> Option<&str> {
        self.inner.response_headers().get_str(name)
    }

    /// Returns the value of a response trailer by name, if present.
    pub fn trailer<'a>(&self, name: impl Into<HeaderName<'a>>) -> Option<&str> {
        self.inner
            .response_trailers()
            .and_then(|trailers| trailers.get_str(name))
    }

    /// Asserts that the response status equals `expected`.
    #[track_caller]
    pub fn assert_status(&self, status: impl TryInto<Status>) -> &Self {
        let expected: Status = status
            .try_into()
            .ok()
            .expect("expected a valid status code");
        let actual = self.status();
        assert_eq!(actual, expected, "expected status {expected}, got {actual}");
        self
    }

    /// Asserts that the response status is 200 OK.
    #[track_caller]
    pub fn assert_ok(&self) -> &Self {
        self.assert_status(200)
    }

    /// Asserts that the response body is a string that equals `expected`, ignoring trailing
    /// whitespace
    #[track_caller]
    pub fn assert_body(&self, expected: &str) -> &Self {
        assert_eq!(self.body().trim_end(), expected.trim_end());
        self
    }

    /// Asserts that the response body contains `pattern`.
    #[track_caller]
    pub fn assert_body_contains(&self, pattern: &str) -> &Self {
        let body = self.body();
        assert!(
            body.contains(pattern),
            "expected body to contain {pattern:?}, but body was:\n{body}"
        );
        self
    }

    /// Asserts that the response has a header `name` with value `value`.
    #[track_caller]
    pub fn assert_header<'a, HV, HN>(&self, name: HN, expected: HV) -> &Self
    where
        HeaderValues: PartialEq<HV>,
        HV: Debug,
        HN: Into<HeaderName<'a>>,
    {
        let name = name.into();

        match self.inner.response_headers().get_values(name.clone()) {
            Some(actual) => assert_eq!(*actual, expected, "for header {name:?}"),
            None => panic!("header {name} not set"),
        };

        self
    }

    /// Asserts that the response has a header `name` with value `value`.
    #[track_caller]
    pub fn assert_headers<'a, I, HN, HV>(&self, headers: I) -> &Self
    where
        I: IntoIterator<Item = (HN, HV)> + Send,
        HN: Into<HeaderName<'a>>,
        HV: Debug,
        HeaderValues: PartialEq<HV>,
    {
        for (name, expected) in headers {
            self.assert_header(name, expected);
        }

        self
    }

    /// Asserts that the response has no header named `name`.
    #[track_caller]
    pub fn assert_no_header(&self, name: &str) -> &Self {
        let actual = self.header(name);
        assert!(
            actual.is_none(),
            "expected no header {name:?}, but found {actual:?}"
        );
        self
    }

    /// Asserts that a header with the given name exists and runs the provided closure with its
    /// value.
    #[track_caller]
    pub fn assert_header_with<'a, F>(&self, name: impl Into<HeaderName<'a>>, f: F) -> &Self
    where
        F: FnOnce(&HeaderValues),
    {
        let name = name.into();
        match self.response_headers().get_values(name.clone()) {
            Some(values) => f(values),
            None => panic!("expected header {name:?}, but it was not found"),
        }

        self
    }

    /// Asserts that handler state of type `T` was set and runs the provided closure with it.
    #[track_caller]
    pub fn assert_state_with<T, F>(&self, f: F) -> &Self
    where
        T: Send + Sync + 'static,
        F: FnOnce(&T),
    {
        match self.state::<T>() {
            Some(state) => f(state),
            None => panic!(
                "expected handler state of type {}, but none was found",
                type_name::<T>()
            ),
        };
        self
    }

    /// Runs the provided closure with the response body.
    #[track_caller]
    pub fn assert_body_with<F>(&self, f: F) -> &Self
    where
        F: FnOnce(&str),
    {
        f(self.body());
        self
    }

    /// Parses the response body as JSON and runs the provided closure with the parsed value.
    #[cfg(any(feature = "sonic-rs", feature = "serde_json"))]
    #[track_caller]
    pub fn assert_json_body_with<T, F>(&self, f: F) -> &Self
    where
        T: serde::de::DeserializeOwned,
        F: FnOnce(&T),
    {
        let parsed: T =
            crate::from_json_str(self.body()).expect("failed to parse response body as JSON");
        f(&parsed);
        self
    }

    /// Parses the response body as JSON and runs the provided closure with the parsed value.
    #[cfg(any(feature = "sonic-rs", feature = "serde_json"))]
    #[track_caller]
    pub fn assert_json_body<T>(&self, body: &T) -> &Self
    where
        T: serde::de::DeserializeOwned + PartialEq + Debug,
    {
        let parsed: T =
            crate::from_json_str(self.body()).expect("failed to parse response body as JSON");
        assert_eq!(&parsed, body);
        self
    }

    /// Asserts that the response has a trailer `name` with value `value`.
    #[track_caller]
    pub fn assert_trailer<'a, HV, HN>(&self, name: HN, expected: HV) -> &Self
    where
        HeaderValues: PartialEq<HV>,
        HV: Debug,
        HN: Into<HeaderName<'a>>,
    {
        let name = name.into();

        match self
            .inner
            .response_trailers()
            .and_then(|trailers| trailers.get_values(name.clone()))
        {
            Some(actual) => assert_eq!(*actual, expected, "for trailer {name:?}"),
            None => panic!("trailer {name} not set"),
        };

        self
    }

    /// Asserts that the response has a trailer `name` with value `value`.
    #[track_caller]
    pub fn assert_trailers<'a, I, HN, HV>(&self, trailers: I) -> &Self
    where
        I: IntoIterator<Item = (HN, HV)> + Send,
        HN: Into<HeaderName<'a>>,
        HV: Debug,
        HeaderValues: PartialEq<HV>,
    {
        for (name, expected) in trailers {
            self.assert_trailer(name, expected);
        }

        self
    }

    /// Asserts that the response has no trailer named `name`.
    #[track_caller]
    pub fn assert_no_trailer(&self, name: &str) -> &Self {
        let actual = self.trailer(name);
        assert!(
            actual.is_none(),
            "expected no trailer {name:?}, but found {actual:?}"
        );
        self
    }

    /// Asserts that a trailer with the given name exists and runs the provided closure with its
    /// value.
    #[track_caller]
    pub fn assert_trailer_with<'a, F>(&self, name: impl Into<HeaderName<'a>>, f: F) -> &Self
    where
        F: FnOnce(&HeaderValues),
    {
        let name = name.into();
        match self
            .response_trailers()
            .and_then(|trailers| trailers.get_values(name.clone()))
        {
            Some(values) => f(values),
            None => panic!("expected trailer {name:?}, but it was not found"),
        }

        self
    }
}

impl IntoFuture for ConnTest {
    type IntoFuture = Pin<Box<dyn Future<Output = ConnTest> + Send + 'static>>;
    type Output = ConnTest;

    fn into_future(mut self) -> Self::IntoFuture {
        Box::pin(async move {
            if let Some(peer_ip) = self.peer_ip.take() {
                let _ = self.peer_ip_sender.send(peer_ip).await;
            }

            if let Some(host) = self
                .inner
                .request_headers()
                .get_str(KnownHeaderName::Host)
                .map(ToString::to_string)
            {
                let _ = self.inner.url_mut().set_host(Some(&host));
            }

            let inner = &mut self.inner;

            inner.await.expect("request to virtual server failed");

            let inner = &mut self.inner;

            if let Some(transport) = inner.transport_mut() {
                let state = std::mem::take(
                    &mut *((transport as &dyn Any)
                        .downcast_ref::<TestTransport>()
                        .unwrap()
                        .state()
                        .write()
                        .unwrap()),
                );

                *inner.as_mut() = state;
            }

            self.body = Some(
                self.inner
                    .response_body()
                    .read_bytes()
                    .await
                    .expect("failed to read response body"),
            );

            self
        })
    }
}
