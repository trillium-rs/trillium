use std::{
    convert::TryInto,
    fmt::Debug,
    net::IpAddr,
    ops::{Deref, DerefMut},
};
use trillium::{Conn, Handler, HeaderName, HeaderValues, Method};
use trillium_http::{Conn as HttpConn, Synthetic};

type SyntheticConn = HttpConn<Synthetic>;

/**
A wrapper around a [`trillium::Conn`] for testing

Stability note: this may be replaced by an extension trait at some point.
*/
#[derive(Debug)]
pub struct TestConn(Conn);

impl TestConn {
    /**
    constructs a new TestConn with the provided method, path, and body.
    ```
    use trillium_testing::{prelude::*, TestConn};
    let mut conn = TestConn::build("get", "/", "body");
    assert_eq!(conn.method(), Method::Get);
    assert_eq!(conn.path(), "/");
    assert_eq!(conn.take_request_body_string(), "body");
    ```
    */
    pub fn build<M>(method: M, path: impl Into<String>, body: impl Into<Synthetic>) -> Self
    where
        M: TryInto<Method>,
        <M as TryInto<Method>>::Error: Debug,
    {
        Self(HttpConn::new_synthetic(method.try_into().unwrap(), path.into(), body).into())
    }

    /**
    chainable constructor to append a request header to the TestConn
    ```
    use trillium_testing::TestConn;
    let conn = TestConn::build("get", "/", "body")
        .with_request_header("some-header", "value");
    assert_eq!(conn.headers().get_str("some-header"), Some("value"));
    ```
    */

    pub fn with_request_header(
        self,
        header_name: impl Into<HeaderName<'static>>,
        header_value: impl Into<HeaderValues>,
    ) -> Self {
        let mut inner: SyntheticConn = self.into();
        inner
            .request_headers_mut()
            .append(header_name, header_value);
        Self(inner.into())
    }

    /**
    chainable constructor to replace the request body. this is useful
    when chaining with a [`trillium_testing::methods`](crate::methods)
    builder, as they do not provide a way to specify the body.

    ```
    use trillium_testing::{methods::post, TestConn};
    let mut conn = post("/").with_request_body("some body");
    assert_eq!(conn.take_request_body_string(), "some body");

    let mut conn = TestConn::build("post", "/", "some body")
        .with_request_body("once told me");
    assert_eq!(conn.take_request_body_string(), "once told me");

    ```
    */
    pub fn with_request_body(self, body: impl Into<Synthetic>) -> Self {
        let mut inner: SyntheticConn = self.into();
        inner.replace_body(body);
        Self(inner.into())
    }

    /// sets the peer ip for this test conn
    pub fn with_peer_ip(mut self, ip: IpAddr) -> Self {
        self.inner_mut().set_peer_ip(Some(ip));
        self
    }

    /// set the test conn to be secure
    pub fn secure(mut self) -> Self {
        self.inner_mut().set_secure(true);
        self
    }

    /// set state on the test conn
    pub fn with_state<S>(mut self, state: S) -> Self
    where
        S: Send + Sync + 'static,
    {
        self.0.set_state(state);
        self
    }

    /**
    blocks on running this conn against a handler and finalizes
    response headers. also aliased as [`TestConn::on`]

    ```
    use trillium_testing::prelude::*;

    async fn handler(conn: Conn) -> Conn {
        conn.ok("hello trillium")
    }

    let conn = get("/").run(&handler);
    assert_ok!(conn, "hello trillium", "content-length" => "14");
    ```
    */
    pub fn run(self, handler: &impl Handler) -> Self {
        crate::block_on(self.run_async(handler))
    }

    /**
    runs this conn against a handler and finalizes
    response headers.

    ```
    use trillium_testing::prelude::*;

    async fn handler(conn: Conn) -> Conn {
        conn.ok("hello trillium")
    }

    block_on(async move {
        let conn = get("/").run_async(&handler).await;
        assert_ok!(conn, "hello trillium", "content-length" => "14");
    });
    ```
    */
    pub async fn run_async(self, handler: &impl Handler) -> Self {
        let conn = handler.run(self.into()).await;
        let mut conn = handler.before_send(conn).await;
        conn.inner_mut().finalize_headers();
        Self(conn)
    }

    /**
    blocks on running this conn against a handler and finalizes
    response headers. also aliased as [`TestConn::run`].

    ```
    use trillium_testing::prelude::*;

    async fn handler(conn: Conn) -> Conn {
        conn.ok("hello trillium")
    }

    let conn = get("/").on(&handler);
    assert_ok!(conn, "hello trillium", "content-length" => "14");
    ```
    */

    pub fn on(self, handler: &impl Handler) -> Self {
        self.run(handler)
    }

    /**
    Reads the response body to string and returns it, if set. This is
    used internally to [`assert_body`] which is the preferred
    interface
    */
    pub fn take_response_body_string(&mut self) -> Option<String> {
        if let Some(body) = self.take_response_body() {
            String::from_utf8(
                futures_lite::future::block_on(body.into_bytes())
                    .unwrap()
                    .to_vec(),
            )
            .ok()
        } else {
            None
        }
    }

    /**
    Reads the request body to string and returns it
    */
    pub fn take_request_body_string(&mut self) -> String {
        futures_lite::future::block_on(async {
            self.request_body().await.read_string().await.unwrap()
        })
    }
}

impl From<Conn> for TestConn {
    fn from(conn: Conn) -> Self {
        Self(conn)
    }
}

impl From<TestConn> for Conn {
    fn from(tc: TestConn) -> Self {
        tc.0
    }
}

impl From<TestConn> for SyntheticConn {
    fn from(tc: TestConn) -> Self {
        tc.0.into_inner()
    }
}

impl Deref for TestConn {
    type Target = Conn;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for TestConn {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
