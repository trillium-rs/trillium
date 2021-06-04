use routefinder::{Match, Router as Routefinder};
use std::{collections::HashMap, sync::Arc};
use trillium::{async_trait, http_types::Method, Conn, Handler, Upgrade};

use crate::RouterRef;
/**
# The Router handler


*/
#[derive(Default, Debug)]
pub struct Router(HashMap<Method, Routefinder<Box<dyn Handler>>>);

macro_rules! method {
    ($fn_name:ident, $method:ident) => {
        method!(
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
let router = Router::new().",
                stringify!($fn_name),
                "(\"/some/route\", |conn: Conn| async move {
  conn.ok(\"success\")
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
        pub fn $fn_name(mut self, path: &'static str, handler: impl Handler) -> Self {
            self.add(path, Method::$method, handler);
            self
        }
    };
}

impl Router {
    /**
    Constructs a new Router. This is often used with [`Router::get`],
    [`Router::post`], [`Router::put`], [`Router::delete`], and
    [`Router::patch`] chainable methods to build up an application.

    For an alternative way of constructing a Router, see [`Router::build`]

    ```
    # use trillium::Conn;
    # use trillium_router::Router;

    let router = Router::new()
        .get("/", |conn: Conn| async move { conn.ok("you have reached the index") })
        .get("/some/:param", |conn: Conn| async move { conn.ok("you have reached /some/:param") })
        .post("/", |conn: Conn| async move { conn.ok("post!") });

    use trillium_testing::{methods::{get, post}, assert_ok};
    assert_ok!(get("/").on(&router), "you have reached the index");
    assert_ok!(get("/some/route").on(&router), "you have reached /some/:param");
    assert_ok!(post("/").on(&router), "post!");
    ```
     */
    pub fn new() -> Self {
        Self::default()
    }

    /**
    Another way to build a router, if you don't like the chainable
    interface described in [`Router::new`]. Note that the argument to
    the closure is a [`RouterRef`].

    ```
    # use trillium::Conn;
    # use trillium_router::Router;
    let router = Router::build(|mut router| {
        router.get("/", |conn: Conn| async move {
            conn.ok("you have reached the index")
        });

        router.get("/some/:paramroute", |conn: Conn| async move {
            conn.ok("you have reached /some/:param")
        });

        router.post("/", |conn: Conn| async move {
            conn.ok("post!")
        });
    });


    use trillium_testing::{methods::{get, post}, assert_ok};
    assert_ok!(get("/").on(&router), "you have reached the index");
    assert_ok!(get("/some/route").on(&router), "you have reached /some/:param");
    assert_ok!(post("/").on(&router), "post!");
    ```
    */
    pub fn build(builder: impl Fn(RouterRef)) -> Router {
        let mut router = Router::new();
        builder(RouterRef::new(&mut router));
        router
    }

    fn best_match<'a, 'b>(
        &'a self,
        method: &Method,
        path: &'b str,
    ) -> Option<Match<'a, 'b, Box<dyn Handler>>> {
        self.0.get(method).and_then(|r| r.best_match(path))
    }

    pub(crate) fn add(&mut self, path: &'static str, method: Method, handler: impl Handler) {
        self.0
            .entry(method)
            .or_insert_with(routefinder::Router::new)
            .add(path, Box::new(handler))
            .expect("could not add route")
    }

    pub(crate) fn add_any(&mut self, path: &'static str, handler: impl Handler) {
        use Method::*;
        let handler = Arc::new(handler);
        for method in &[Get, Post, Put, Delete, Patch] {
            self.add(path, *method, handler.clone())
        }
    }

    /**
    Appends the handler to all (get, post, put, delete, and patch) methods.
    ```
    # use trillium::Conn;
    # use trillium_router::Router;
    let router = Router::new().any("/any", |conn: Conn| async move {
        let response = format!("you made a {} request to /any", conn.method());
        conn.ok(response)
    });

    use trillium_testing::{assert_ok, methods::*, assert_not_handled};
    assert_ok!(get("/any").on(&router), "you made a GET request to /any");
    assert_ok!(post("/any").on(&router), "you made a POST request to /any");
    assert_ok!(delete("/any").on(&router), "you made a DELETE request to /any");
    assert_ok!(patch("/any").on(&router), "you made a PATCH request to /any");
    assert_ok!(put("/any").on(&router), "you made a PUT request to /any");

    assert_not_handled!(get("/").on(&router));
    ```
    */
    pub fn any(mut self, path: &'static str, handler: impl Handler) -> Self {
        self.add_any(path, handler);
        self
    }

    method!(get, Get);
    method!(post, Post);
    method!(put, Put);
    method!(delete, Delete);
    method!(patch, Patch);
}

#[async_trait]
impl Handler for Router {
    async fn run(&self, conn: Conn) -> Conn {
        if let Some(m) = self.best_match(&conn.method(), conn.path()) {
            let captures = m.captures().into_owned();
            struct HasPath;
            log::debug!("running {}: {}", m.route(), m.name());
            let mut new_conn = m
                .handler()
                .run({
                    let mut conn = conn;
                    if let Some(wildcard) = captures.wildcard() {
                        conn.push_path(String::from(wildcard));
                        conn.set_state(HasPath);
                    }
                    conn.with_state(captures)
                })
                .await;
            if new_conn.take_state::<HasPath>().is_some() {
                new_conn.pop_path();
            }
            new_conn
        } else {
            log::debug!("{} did not match any route", conn.path());
            conn
        }
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        if let Some(m) = self.best_match(&conn.method(), conn.path()) {
            m.handler().before_send(conn).await
        } else {
            conn
        }
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        if let Some(m) = self.best_match(&upgrade.method(), upgrade.path()) {
            m.handler().has_upgrade(upgrade)
        } else {
            false
        }
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        self.best_match(&upgrade.method(), upgrade.path())
            .unwrap()
            .handler()
            .upgrade(upgrade)
            .await
    }
}
