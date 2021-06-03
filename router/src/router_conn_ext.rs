use routefinder::Captures;
use trillium::Conn;

/**
Extends trillium::Conn with accessors for router params.
*/
pub trait RouterConnExt {
    /**
    Retrieves a captured param from the conn. Note that this will only
    be Some if the exact param is present in the matched route.

    Routefinder params are defined starting with a colon, but the
    colon is not needed when fetching the param.

    ```
    use trillium::{conn_unwrap, Conn};
    use trillium_router::{Router, RouterConnExt};

    let router = Router::new().get("/pages/:page_name", |conn: Conn| async move {
        let page_name = conn_unwrap!(conn, conn.param("page_name"));
        let content = format!("you have reached the page named {}", page_name);
        conn.ok(content)
    });

    use trillium_testing::{methods::get, assert_ok};
    assert_ok!(get(&router, "/pages/trillium"), "you have reached the page named trillium");
    ```
    */

    fn param<'a>(&'a self, param: &str) -> Option<&'a str>;

    /// Retrieves the wildcard match from the conn. Note that this will
    /// only be Some if the matched route contains a wildcard, as
    /// expressed by a "*" in the routefinder route spec.
    ///
    /// ```
    /// use trillium::{conn_unwrap, Conn};
    /// use trillium_router::{Router, RouterConnExt};
    ///
    /// let router = Router::new().get("/pages/*", |conn: Conn| async move {
    ///     let wildcard = conn_unwrap!(conn, conn.wildcard());
    ///     let content = format!("the wildcard matched {}", wildcard);
    ///     conn.ok(content)
    /// });
    ///
    /// use trillium_testing::{methods::get, assert_ok};
    /// assert_ok!(
    ///     get(&router, "/pages/this/is/a/wildcard/match"),
    ///     "the wildcard matched this/is/a/wildcard/match"
    /// );
    /// ```

    fn wildcard(&self) -> Option<&str>;
}

impl RouterConnExt for Conn {
    fn param<'a>(&'a self, param: &str) -> Option<&'a str> {
        self.state::<Captures>().and_then(|p| p.get(param))
    }

    fn wildcard(&self) -> Option<&str> {
        self.state::<Captures>().and_then(|p| p.wildcard())
    }
}

// ```
// use trillium::{conn_unwrap, Conn};
// use trillium_router::{Router, RouterConnExt};

// let router = Router::new().get("/pages/*", |conn: Conn| async move {
//     let wildcard = conn_unwrap!(conn, conn.wildcard());
//     let content = format!("the wildcard matched {}", wildcard);
//     conn.ok(content)
// });

// use trillium_testing::{HandlerTesting, assert_ok};
// assert_ok!(
//     router.get("/pages/this/is/a/wildcard/match"),
//     "the wildcard matched this/is/a/wildcard/match"
// );
// ```
