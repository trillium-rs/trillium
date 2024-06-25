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
let router = Router::build(|mut router| {
    router.",
                stringify!($fn_name),
                "(\"/some/route\", |conn: Conn| async move {
        conn.ok(\"success\")
    });
});

use trillium_testing::{methods::",
                stringify!($fn_name),
                ", assert_ok, assert_not_handled};
assert_ok!(",
                stringify!($fn_name),
                "(\"/some/route\").on(&router), \"success\");
assert_not_handled!(",
                stringify!($fn_name),
                "(\"/other/route\").on(&router));
```
"
            )
        );
    };

    ($fn_name:ident, $method:ident, $doc_comment:expr) => {
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
    /// let router = Router::build(|mut router| {
    ///     router.all("/any", |conn: Conn| async move {
    ///         let response = format!("you made a {} request to /any", conn.method());
    ///         conn.ok(response)
    ///     });
    /// });
    ///
    /// use trillium_testing::prelude::*;
    /// assert_ok!(get("/any").on(&router), "you made a GET request to /any");
    /// assert_ok!(post("/any").on(&router), "you made a POST request to /any");
    /// assert_ok!(
    ///     delete("/any").on(&router),
    ///     "you made a DELETE request to /any"
    /// );
    /// assert_ok!(
    ///     patch("/any").on(&router),
    ///     "you made a PATCH request to /any"
    /// );
    /// assert_ok!(put("/any").on(&router), "you made a PUT request to /any");
    ///
    /// assert_not_handled!(get("/").on(&router));
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
    /// let router = Router::build(|mut router| {
    ///     router.any(&["get", "post"], "/get_or_post", |conn: Conn| async move {
    ///         let response = format!("you made a {} request to /get_or_post", conn.method());
    ///         conn.ok(response)
    ///     });
    /// });
    ///
    /// use trillium_testing::prelude::*;
    /// assert_ok!(
    ///     get("/get_or_post").on(&router),
    ///     "you made a GET request to /get_or_post"
    /// );
    /// assert_ok!(
    ///     post("/get_or_post").on(&router),
    ///     "you made a POST request to /get_or_post"
    /// );
    /// assert_not_handled!(delete("/any").on(&router));
    /// assert_not_handled!(patch("/any").on(&router));
    /// assert_not_handled!(put("/any").on(&router));
    /// assert_not_handled!(get("/").on(&router));
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
    /// use trillium_testing::{prelude::*, TestConn};
    /// assert_ok!(
    ///     TestConn::build(Method::Options, "/some/route", ()).on(&router),
    ///     "directly handling options"
    /// );
    /// assert_ok!(
    ///     TestConn::build("checkin", "/some/route", ()).on(&router),
    ///     "checkin??"
    /// );
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
