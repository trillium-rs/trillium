use crate::RouterRef;
use routefinder::{Match, RouteSpec, Router as Routefinder};
use std::{
    collections::{BTreeSet, HashMap},
    convert::TryInto,
    fmt::{self, Debug, Formatter},
    mem,
    sync::Arc,
};
use trillium::{async_trait, Conn, Handler, Info, KnownHeaderName, Method, Upgrade};

const ALL_METHODS: [Method; 5] = [
    Method::Delete,
    Method::Get,
    Method::Patch,
    Method::Post,
    Method::Put,
];
/**
# The Router handler

See crate level docs for more, as this is the primary type in this crate.

*/
pub struct Router {
    all_methods: Routefinder<Box<dyn Handler>>,
    method_map: HashMap<Method, Routefinder<Box<dyn Handler>>>,
    handle_options: bool,
}

impl Default for Router {
    fn default() -> Self {
        Self {
            all_methods: Routefinder::default(),
            method_map: Default::default(),
            handle_options: true,
        }
    }
}

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

    use trillium_testing::prelude::*;
    assert_ok!(get("/").on(&router), "you have reached the index");
    assert_ok!(get("/some/route").on(&router), "you have reached /some/:param");
    assert_ok!(post("/").on(&router), "post!");
    ```
     */
    pub fn new() -> Self {
        Self::default()
    }

    /**
    Disable the default behavior of responding to OPTIONS requests
    with the supported methods at a given path
    */
    pub fn without_options_handling(mut self) -> Self {
        self.set_options_handling(false);
        self
    }

    /**
    enable or disable the router's behavior of responding to OPTIONS requests with the supported methods at given path.

    default: enabled
     */
    pub(crate) fn set_options_handling(&mut self, options_enabled: bool) {
        self.handle_options = options_enabled;
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


    use trillium_testing::prelude::*;
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
        let from_method_map = self.method_map.get(method).and_then(|r| r.best_match(path));
        let from_all_methods = self.all_methods.best_match(path);
        match (from_method_map, from_all_methods) {
            (None, None) => None,
            (None, Some(x)) => Some(x),
            (Some(x), None) => Some(x),
            (Some(x), Some(y)) => {
                if x.route() < y.route() {
                    Some(x)
                } else {
                    Some(y)
                }
            }
        }
    }

    /**
    Registers a handler for a method other than get, put, post, patch, or delete.

    ```
    # use trillium::{Conn, Method};
    # use trillium_router::Router;
    let router = Router::new()
        .with_route("OPTIONS", "/some/route", |conn: Conn| async move { conn.ok("directly handling options") })
        .with_route(Method::Checkin, "/some/route", |conn: Conn| async move { conn.ok("checkin??") });

    use trillium_testing::{prelude::*, TestConn};
    assert_ok!(TestConn::build(Method::Options, "/some/route", ()).on(&router), "directly handling options");
    assert_ok!(TestConn::build("checkin", "/some/route", ()).on(&router), "checkin??");
    ```
    */
    pub fn with_route<M>(mut self, method: M, path: &'static str, handler: impl Handler) -> Self
    where
        M: TryInto<Method>,
        <M as TryInto<Method>>::Error: Debug,
    {
        self.add(path, method.try_into().unwrap(), handler);
        self
    }

    pub(crate) fn add(&mut self, path: &'static str, method: Method, handler: impl Handler) {
        self.method_map
            .entry(method)
            .or_insert_with(routefinder::Router::new)
            .add(path, Box::new(handler))
            .expect("could not add route")
    }

    pub(crate) fn add_any(
        &mut self,
        methods: &[Method],
        path: &'static str,
        handler: impl Handler,
    ) {
        let handler = Arc::new(handler);
        for method in methods {
            self.add(path, *method, handler.clone())
        }
    }

    pub(crate) fn add_all(&mut self, path: &'static str, handler: impl Handler) {
        self.all_methods
            .add(path, Box::new(handler))
            .expect("could not add route");
    }

    /**
    Appends the handler to all (get, post, put, delete, and patch) methods.
    ```
    # use trillium::Conn;
    # use trillium_router::Router;
    let router = Router::new().all("/any", |conn: Conn| async move {
        let response = format!("you made a {} request to /any", conn.method());
        conn.ok(response)
    });

    use trillium_testing::prelude::*;
    assert_ok!(get("/any").on(&router), "you made a GET request to /any");
    assert_ok!(post("/any").on(&router), "you made a POST request to /any");
    assert_ok!(delete("/any").on(&router), "you made a DELETE request to /any");
    assert_ok!(patch("/any").on(&router), "you made a PATCH request to /any");
    assert_ok!(put("/any").on(&router), "you made a PUT request to /any");

    assert_not_handled!(get("/").on(&router));
    ```
    */
    pub fn all(mut self, path: &'static str, handler: impl Handler) -> Self {
        self.add_all(path, handler);
        self
    }

    /**
    Appends the handler to each of the provided http methods.
    ```
    # use trillium::Conn;
    # use trillium_router::Router;
    let router = Router::new().any(&["get", "post"], "/get_or_post", |conn: Conn| async move {
        let response = format!("you made a {} request to /get_or_post", conn.method());
        conn.ok(response)
    });

    use trillium_testing::prelude::*;
    assert_ok!(get("/get_or_post").on(&router), "you made a GET request to /get_or_post");
    assert_ok!(post("/get_or_post").on(&router), "you made a POST request to /get_or_post");
    assert_not_handled!(delete("/any").on(&router));
    assert_not_handled!(patch("/any").on(&router));
    assert_not_handled!(put("/any").on(&router));
    assert_not_handled!(get("/").on(&router));
    ```
    */
    pub fn any<IntoMethod>(
        mut self,
        methods: &[IntoMethod],
        path: &'static str,
        handler: impl Handler,
    ) -> Self
    where
        IntoMethod: TryInto<Method> + Clone,
        <IntoMethod as TryInto<Method>>::Error: Debug,
    {
        let methods = methods
            .to_vec()
            .into_iter()
            .map(|m| m.try_into().unwrap())
            .collect::<Vec<_>>();
        self.add_any(&methods, path, handler);
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
        let method = conn.method();
        let path = conn.path();

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
        } else if method == Method::Options && self.handle_options {
            let mut methods_set: BTreeSet<Method> = if path == "*" {
                if self.all_methods.is_empty() {
                    self.method_map.keys().copied().collect()
                } else {
                    ALL_METHODS.into_iter().collect()
                }
            } else if self.all_methods.best_match(path).is_some() {
                ALL_METHODS.into_iter().collect()
            } else {
                self.method_map
                    .iter()
                    .filter_map(|(m, router)| router.best_match(path).map(|_| *m))
                    .collect()
            };

            methods_set.remove(&Method::Options);

            let allow = methods_set
                .iter()
                .map(|m| m.to_string())
                .collect::<Vec<_>>()
                .join(", ");

            return conn
                .with_header(KnownHeaderName::Allow, allow)
                .with_status(200)
                .halt();
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
        if let Some(m) = self.best_match(upgrade.method(), upgrade.path()) {
            m.handler().has_upgrade(upgrade)
        } else {
            false
        }
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        self.best_match(upgrade.method(), upgrade.path())
            .unwrap()
            .handler()
            .upgrade(upgrade)
            .await
    }

    fn name(&self) -> std::borrow::Cow<'static, str> {
        format!("{:#?}", &self).into()
    }

    async fn init(&mut self, info: &mut Info) {
        // This code is not what a reader would expect, so here's a
        // brief explanation:
        //
        // Currently, the init trait interface must return a Send
        // future because that's the default for async-trait. We don't
        // actually need it to be Send, but changing that would be a
        // semver-minor trillium release.
        //
        // Mutable map iterators are not Send, and because we need to
        // hold that data across await boundaries, we cannot mutate in
        // place.
        //
        // However, because this is only called once at app boot, and
        // because we have &mut self, it is safe to move the router
        // contents into this future and then replace it, and the
        // performance impacts of doing so are unimportant as it is
        // part of app boot.
        let all_methods = mem::take(&mut self.all_methods);
        for (route, mut handler) in all_methods {
            handler.init(info).await;
            self.all_methods.add(route, handler).unwrap();
        }

        let method_map = mem::take(&mut self.method_map);
        for (method, router) in method_map {
            let mut new_router = Routefinder::new();
            for (route, mut handler) in router {
                handler.init(info).await;
                new_router.add(route, handler).unwrap();
            }

            self.method_map.insert(method, new_router);
        }
    }
}

struct RouteForDisplay<'a, H>(Option<&'a Method>, &'a RouteSpec, &'a H);
impl<'a, H: Handler> Debug for RouteForDisplay<'a, H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            RouteForDisplay(Some(method), route, handler) => {
                f.write_fmt(format_args!("{} {} -> {}", method, route, handler.name()))
            }
            RouteForDisplay(None, route, handler) => {
                f.write_fmt(format_args!("* {} -> {}", route, handler.name()))
            }
        }
    }
}

impl Debug for Router {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("Router ")?;
        let mut set = f.debug_set();

        for (route, handler) in &self.all_methods {
            set.entry(&RouteForDisplay(None, route, handler));
        }

        for (method, router) in &self.method_map {
            for (route, handler) in router {
                set.entry(&RouteForDisplay(Some(method), route, handler));
            }
        }

        set.finish()
    }
}
