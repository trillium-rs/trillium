#![allow(clippy::missing_panics_doc)]
use crate::TestTransport;
use std::{
    any::{Any, type_name},
    fmt::{self, Debug, Formatter},
    future::{Future, IntoFuture},
    net::SocketAddr,
    pin::Pin,
    str,
    sync::Arc,
};
use trillium_client::{Client, Connector, IntoUrl};
use trillium_http::{
    Conn, HeaderName, HeaderValues, Headers, KnownHeaderName, Method, ServerConfig, Status,
};

/// A test server for the http crate that runs a http/1.1 client over a virtual in-memory transport,
/// similar to [`crate::TestServer`].
///
/// Note that this is not intended to be used outside of testing the `trillium-http` crate and you
/// probably want to use [`TestServer`](crate::TestServer)
#[derive(Clone, Debug)]
pub struct HttpTest<H> {
    client: Client,
    connector: TestConnector<H>,
}

#[derive(Debug)]
struct TestConnector<H>(Arc<ServerConfig>, Arc<H>, crate::Runtime);

impl<H> Clone for TestConnector<H> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), self.1.clone(), self.2.clone())
    }
}

impl<Fun, Fut> Connector for TestConnector<Fun>
where
    Fun: Fn(Conn<TestTransport>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Conn<TestTransport>> + Send + 'static,
{
    type Runtime = crate::Runtime;
    type Transport = TestTransport;
    type Udp = ();

    async fn connect(&self, url: &trillium_client::Url) -> std::io::Result<Self::Transport> {
        let secure = url.scheme() == "https";
        let (client_transport, server_transport) = TestTransport::new();
        let TestConnector(server_config, handler, runtime) = self.clone();

        runtime.spawn(async move {
            server_config
                .run(server_transport, |mut conn| async {
                    conn.set_secure(secure);
                    (handler)(conn).await
                })
                .await
                .unwrap();
        });
        Ok(client_transport)
    }

    fn runtime(&self) -> Self::Runtime {
        self.2.clone()
    }

    async fn resolve(&self, _host: &str, _port: u16) -> std::io::Result<Vec<SocketAddr>> {
        Ok(vec![SocketAddr::from(([0, 0, 0, 0], 0))])
    }
}

impl<Fun, Fut> HttpTest<Fun>
where
    Fun: Fn(Conn<TestTransport>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Conn<TestTransport>> + Send + 'static,
{
    /// Creates a new [`TestServer`], running [`init`](crate::init) on the handler.
    pub fn new(handler: Fun) -> Self {
        let connector = TestConnector(
            Arc::new(ServerConfig::new()),
            Arc::new(handler),
            crate::runtime().into(),
        );
        let client = Client::new(connector.clone()).with_base("http://trillium.test/");

        Self { client, connector }
    }

    /// Build a new [`ConnTest`]
    pub fn build<M: TryInto<Method>>(&self, method: M, path: &str) -> ConnTest
    where
        M::Error: Debug,
    {
        ConnTest {
            inner: self.client.build_conn(method, path),
            body: None,
            runtime: self.connector.2.clone(),
        }
    }

    /// borrow from shared state configured by the handler
    pub fn shared_state<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.connector.0.shared_state().get()
    }

    /// Borrow the handler
    pub fn handler(&self) -> &Fun {
        &self.connector.1
    }

    /// Add a default host/authority for this virtual server (eg pretend this server is running at
    /// "example.com" with `.with_host("example.com")`
    #[must_use]
    pub fn with_host(mut self, host: &str) -> Self {
        self.set_host(host);
        self
    }

    /// Set the default host/authority for this virtual server (eg pretend this server is running at
    /// "example.com" with `.set_host("example.com")`
    pub fn set_host(&mut self, host: &str) -> &mut Self {
        if let Some(base) = self.client.base_mut() {
            let _ = base.set_host(Some(host));
        };
        self
    }

    /// Set the url for this virtual server (eg pretend this server is running at
    /// `https://example.com` with `.with_base("https://example.com")`
    #[must_use]
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

pub struct ConnTest {
    inner: trillium_client::Conn,
    body: Option<Vec<u8>>,
    runtime: crate::Runtime,
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

    /// Returns response trailers, if any were received.
    ///
    /// Only populated after the response body has been fully read (i.e., after `.await` or
    /// `.block()`). Returns `None` when the server sent no trailers or the response was not
    /// chunked.
    pub fn response_trailers(&self) -> Option<&Headers> {
        self.inner.response_trailers()
    }

    /// Returns the value of a response header by name, if present.
    pub fn header<'a>(&self, name: impl Into<HeaderName<'a>>) -> Option<&str> {
        self.inner.response_headers().get_str(name)
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

    /// Execute the conn in a blocking manner
    pub fn block(self) -> Self {
        self.runtime.clone().block_on(self.into_future())
    }
}

impl IntoFuture for ConnTest {
    type IntoFuture = Pin<Box<dyn Future<Output = ConnTest> + Send + 'static>>;
    type Output = ConnTest;

    fn into_future(mut self) -> Self::IntoFuture {
        Box::pin(async move {
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
