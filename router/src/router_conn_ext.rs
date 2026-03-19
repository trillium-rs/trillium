use crate::{CapturesNewType, RouteSpecNewType};
use trillium::Conn;

/// Extends trillium::Conn with accessors for router params.
pub trait RouterConnExt {
    /// Retrieves a captured param from the conn. Note that this will only
    /// be Some if the exact param is present in the matched route.
    ///
    /// Routefinder params are defined starting with a colon, but the
    /// colon is not needed when fetching the param.
    ///
    /// ```
    /// use trillium::{Conn, conn_unwrap};
    /// use trillium_router::{Router, RouterConnExt};
    /// use trillium_testing::TestHandler;
    ///
    /// # trillium_testing::block_on(async {
    /// let router = Router::new().get("/pages/:page_name", |conn: Conn| async move {
    ///     let page_name = conn_unwrap!(conn.param("page_name"), conn);
    ///     let content = format!("you have reached the page named {}", page_name);
    ///     conn.ok(content)
    /// });
    ///
    /// let app = TestHandler::new(router).await;
    /// app.get("/pages/trillium")
    ///     .await
    ///     .assert_ok()
    ///     .assert_body("you have reached the page named trillium");
    /// # });
    /// ```
    fn param<'a>(&'a self, param: &str) -> Option<&'a str>;

    /// Retrieves the wildcard match from the conn. Note that this will
    /// only be Some if the matched route contains a wildcard, as
    /// expressed by a "*" in the routefinder route spec.
    ///
    /// ```
    /// use trillium::{Conn, conn_unwrap};
    /// use trillium_router::{Router, RouterConnExt};
    /// use trillium_testing::TestHandler;
    ///
    /// # trillium_testing::block_on(async {
    /// let router = Router::new().get("/pages/*", |conn: Conn| async move {
    ///     let wildcard = conn_unwrap!(conn.wildcard(), conn);
    ///     let content = format!("the wildcard matched {}", wildcard);
    ///     conn.ok(content)
    /// });
    ///
    /// let app = TestHandler::new(router).await;
    /// app.get("/pages/this/is/a/wildcard/match")
    ///     .await
    ///     .assert_ok()
    ///     .assert_body("the wildcard matched this/is/a/wildcard/match");
    /// # });
    /// ```
    fn wildcard(&self) -> Option<&str>;

    /// Retrieves the matched route specification
    /// ```
    /// use trillium::{Conn, conn_unwrap};
    /// use trillium_router::{Router, RouterConnExt};
    /// use trillium_testing::TestHandler;
    ///
    /// # trillium_testing::block_on(async {
    /// let router = Router::new().get("/pages/:page_id", |conn: Conn| async move {
    ///     let route = conn_unwrap!(conn.route(), conn).to_string();
    ///     conn.ok(format!("route was {route}"))
    /// });
    ///
    /// let app = TestHandler::new(router).await;
    /// app.get("/pages/12345")
    ///     .await
    ///     .assert_ok()
    ///     .assert_body("route was /pages/:page_id");
    /// # });
    /// ```
    fn route(&self) -> Option<&str>;
}

impl RouterConnExt for Conn {
    fn param<'a>(&'a self, param: &str) -> Option<&'a str> {
        self.state().and_then(|CapturesNewType(p)| p.get(param))
    }

    fn wildcard(&self) -> Option<&str> {
        self.state().and_then(|CapturesNewType(p)| p.wildcard())
    }

    fn route(&self) -> Option<&str> {
        self.state().and_then(|RouteSpecNewType(r)| r.source())
    }
}
