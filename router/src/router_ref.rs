use crate::Router;
use routefinder::RouteSpec;
use std::fmt::Debug;
use trillium::{Handler, Method};

macro_rules! method_ref {
    ($fn_name:ident, $method:ident) => {
        method_ref!(
            $fn_name,
            $method,
            concat!(
                // yep, macro-generated doctests
                "Registers a handler for the ",
                stringify!($fn_name),
                " http method.

```
# use trillium::Conn;
# use trillium_router::Router;
# use trillium_testing::TestServer;
# trillium_testing::block_on(async {
let router = Router::build(|mut router| {
    router.",
                stringify!($fn_name),
                "(\"/some/route\", |conn: Conn| async move {
        conn.ok(\"success\")
    });
});

let app = TestServer::new(router).await;
app.",
                stringify!($fn_name),
                "(\"/some/route\").await
    .assert_ok()
    .assert_body(\"success\");
app.",
                stringify!($fn_name),
                "(\"/other/route\").await
    .assert_status(404);
# });
```
"
            )
        );
    };

    ($fn_name:ident, $method:ident, $doc_comment:expr_2021) => {
        #[doc = $doc_comment]
        pub fn $fn_name<R>(&mut self, path: R, handler: impl Handler)
        where
            R: TryInto<RouteSpec>,
            R::Error: Debug,
        {
            self.0.add(path, Method::$method, handler);
        }
    };
}

/// # A `&mut Router` for use with `Router::build`
///
/// A wrapper around a `&mut Router` that supports imperative route
/// registration. See [`Router::build`] for further documentation.
#[derive(Debug)]
pub struct RouterRef<'r>(&'r mut Router);
impl<'r> RouterRef<'r> {
    method_ref!(get, Get);

    method_ref!(post, Post);

    method_ref!(put, Put);

    method_ref!(delete, Delete);

    method_ref!(patch, Patch);

    /// Appends the handler to all (get, post, put, delete, and patch) methods.
    ///
    /// ```
    /// # use trillium::Conn;
    /// # use trillium_router::Router;
    /// # use trillium_testing::TestServer;
    /// # trillium_testing::block_on(async {
    /// let router = Router::build(|mut router| {
    ///     router.all("/any", |conn: Conn| async move {
    ///         let response = format!("you made a {} request to /any", conn.method());
    ///         conn.ok(response)
    ///     });
    /// });
    ///
    /// let app = TestServer::new(router).await;
    /// app.get("/any")
    ///     .await
    ///     .assert_ok()
    ///     .assert_body("you made a GET request to /any");
    /// app.post("/any")
    ///     .await
    ///     .assert_ok()
    ///     .assert_body("you made a POST request to /any");
    /// app.delete("/any")
    ///     .await
    ///     .assert_ok()
    ///     .assert_body("you made a DELETE request to /any");
    /// app.patch("/any")
    ///     .await
    ///     .assert_ok()
    ///     .assert_body("you made a PATCH request to /any");
    /// app.put("/any")
    ///     .await
    ///     .assert_ok()
    ///     .assert_body("you made a PUT request to /any");
    /// app.get("/").await.assert_status(404);
    /// # });
    /// ```
    pub fn all<R>(&mut self, path: R, handler: impl Handler)
    where
        R: TryInto<RouteSpec>,
        R::Error: Debug,
    {
        self.0.add_all(path, handler)
    }

    /// Appends the handler to each of the provided http methods.
    /// ```
    /// # use trillium::Conn;
    /// # use trillium_router::Router;
    /// # use trillium_testing::TestServer;
    /// # trillium_testing::block_on(async {
    /// let router = Router::build(|mut router| {
    ///     router.any(&["get", "post"], "/get_or_post", |conn: Conn| async move {
    ///         let response = format!("you made a {} request to /get_or_post", conn.method());
    ///         conn.ok(response)
    ///     });
    /// });
    ///
    /// let app = TestServer::new(router).await;
    /// app.get("/get_or_post")
    ///     .await
    ///     .assert_ok()
    ///     .assert_body("you made a GET request to /get_or_post");
    /// app.post("/get_or_post")
    ///     .await
    ///     .assert_ok()
    ///     .assert_body("you made a POST request to /get_or_post");
    /// app.delete("/any").await.assert_status(404);
    /// app.patch("/any").await.assert_status(404);
    /// app.put("/any").await.assert_status(404);
    /// app.get("/").await.assert_status(404);
    /// # });
    /// ```
    pub fn any<IntoMethod, R>(&mut self, methods: &[IntoMethod], path: R, handler: impl Handler)
    where
        R: TryInto<RouteSpec>,
        R::Error: Debug,
        IntoMethod: TryInto<Method> + Clone,
        <IntoMethod as TryInto<Method>>::Error: Debug,
    {
        let methods = methods
            .iter()
            .cloned()
            .map(|m| m.try_into().unwrap())
            .collect::<Vec<_>>();

        self.0.add_any(&methods, path, handler);
    }

    pub(crate) fn new(router: &'r mut Router) -> Self {
        Self(router)
    }

    /// Registers a handler for a method other than get, put, post, patch, or delete.
    /// ```
    /// # use trillium::{Conn, Method};
    /// # use trillium_router::Router;
    /// # use trillium_testing::TestServer;
    /// # trillium_testing::block_on(async {
    /// let router = Router::build(|mut router| {
    ///     router.add_route("OPTIONS", "/some/route", |conn: Conn| async move {
    ///         conn.ok("directly handling options")
    ///     });
    ///
    ///     router.add_route(Method::Checkin, "/some/route", |conn: Conn| async move {
    ///         conn.ok("checkin??")
    ///     });
    /// });
    ///
    /// let app = TestServer::new(router).await;
    /// app.build(Method::Options, "/some/route")
    ///     .await
    ///     .assert_ok()
    ///     .assert_body("directly handling options");
    /// app.build(Method::Checkin, "/some/route")
    ///     .await
    ///     .assert_ok()
    ///     .assert_body("checkin??");
    /// # });
    /// ```
    pub fn add_route<M, R>(&mut self, method: M, path: R, handler: impl Handler)
    where
        M: TryInto<Method>,
        <M as TryInto<Method>>::Error: Debug,
        R: TryInto<RouteSpec>,
        R::Error: Debug,
    {
        self.0.add(path, method.try_into().unwrap(), handler);
    }

    /// enable or disable the router's behavior of responding to OPTIONS
    /// requests with the supported methods at given path.
    ///
    /// default: enabled
    ///
    /// see crate-level docs for further explanation of the default behavior.
    pub fn set_options_handling(&mut self, options_enabled: bool) {
        self.0.set_options_handling(options_enabled);
    }
}
