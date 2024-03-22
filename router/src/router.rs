use crate::{CapturesNewType, RouteSpecNewType, RouterRef};
use routefinder::{Match, RouteSpec, Router as Routefinder};
use std::{
    collections::BTreeSet,
    fmt::{self, Debug, Display, Formatter},
    mem,
};
use trillium::{async_trait, Conn, Handler, Info, KnownHeaderName, Method, Upgrade};

const ALL_METHODS: [Method; 5] = [
    Method::Delete,
    Method::Get,
    Method::Patch,
    Method::Post,
    Method::Put,
];

#[derive(Debug)]
enum MethodSelection {
    Just(Method),
    All,
    Any(Vec<Method>),
}

impl Display for MethodSelection {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            MethodSelection::Just(m) => Display::fmt(m, f),
            MethodSelection::All => f.write_str("*"),
            MethodSelection::Any(v) => {
                f.write_str(&v.iter().map(|m| m.as_ref()).collect::<Vec<_>>().join(", "))
            }
        }
    }
}

impl PartialEq<Method> for MethodSelection {
    fn eq(&self, other: &Method) -> bool {
        match self {
            MethodSelection::Just(m) => m == other,
            MethodSelection::All => true,
            MethodSelection::Any(v) => v.contains(other),
        }
    }
}

impl From<()> for MethodSelection {
    fn from(_: ()) -> MethodSelection {
        Self::All
    }
}

impl From<Method> for MethodSelection {
    fn from(method: Method) -> Self {
        Self::Just(method)
    }
}

impl From<&[Method]> for MethodSelection {
    fn from(methods: &[Method]) -> Self {
        Self::Any(methods.to_vec())
    }
}
impl From<Vec<Method>> for MethodSelection {
    fn from(methods: Vec<Method>) -> Self {
        Self::Any(methods)
    }
}

#[derive(Debug, Default)]
struct MethodRoutefinder(Routefinder<(MethodSelection, Box<dyn Handler>)>);
impl MethodRoutefinder {
    fn add<R>(
        &mut self,
        method_selection: impl Into<MethodSelection>,
        path: R,
        handler: impl Handler,
    ) where
        R: TryInto<RouteSpec>,
        R::Error: Debug,
    {
        self.0
            .add(path, (method_selection.into(), Box::new(handler)))
            .expect("could not add route")
    }

    fn methods_matching(&self, path: &str) -> BTreeSet<Method> {
        let mut set = BTreeSet::new();

        fn extend(ms: &MethodSelection, set: &mut BTreeSet<Method>) {
            match ms {
                MethodSelection::All => {
                    set.extend(ALL_METHODS);
                }
                MethodSelection::Just(method) => {
                    set.insert(*method);
                }
                MethodSelection::Any(methods) => {
                    set.extend(methods);
                }
            }
        }

        if path == "*" {
            for ms in self.0.iter().map(|(_, (m, _))| m) {
                extend(ms, &mut set);
            }
        } else {
            for m in self.0.match_iter(path) {
                extend(&m.0, &mut set);
            }
        };

        set.remove(&Method::Options);
        set
    }

    fn best_match<'a, 'b>(
        &'a self,
        method: Method,
        path: &'b str,
    ) -> Option<Match<'a, 'b, (MethodSelection, Box<dyn Handler>)>> {
        self.0.match_iter(path).find(|m| m.0 == method)
    }
}

/**
# The Router handler

See crate level docs for more, as this is the primary type in this crate.

*/
pub struct Router {
    routefinder: MethodRoutefinder,
    handle_options: bool,
}

impl Default for Router {
    fn default() -> Self {
        Self {
            routefinder: MethodRoutefinder::default(),
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
        pub fn $fn_name<R>(mut self, path: R, handler: impl Handler) -> Self
        where
            R: TryInto<RouteSpec>,
            R::Error: Debug,
        {
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
        method: Method,
        path: &'b str,
    ) -> Option<Match<'a, 'b, (MethodSelection, Box<dyn Handler>)>> {
        self.routefinder.best_match(method, path)
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
    pub fn with_route<M, R>(mut self, method: M, path: R, handler: impl Handler) -> Self
    where
        M: TryInto<Method>,
        <M as TryInto<Method>>::Error: Debug,
        R: TryInto<RouteSpec>,
        R::Error: Debug,
    {
        self.add(path, method.try_into().unwrap(), handler);
        self
    }

    pub(crate) fn add<R>(&mut self, path: R, method: Method, handler: impl Handler)
    where
        R: TryInto<RouteSpec>,
        R::Error: Debug,
    {
        self.routefinder.add(method, path, handler);
    }

    pub(crate) fn add_any<R>(&mut self, methods: &[Method], path: R, handler: impl Handler)
    where
        R: TryInto<RouteSpec>,
        R::Error: Debug,
    {
        self.routefinder.add(methods, path, handler)
    }

    pub(crate) fn add_all<R>(&mut self, path: R, handler: impl Handler)
    where
        R: TryInto<RouteSpec>,
        R::Error: Debug,
    {
        self.routefinder.add((), path, handler);
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
    pub fn all<R>(mut self, path: R, handler: impl Handler) -> Self
    where
        R: TryInto<RouteSpec>,
        R::Error: Debug,
    {
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
    pub fn any<IntoMethod, R>(
        mut self,
        methods: &[IntoMethod],
        path: R,
        handler: impl Handler,
    ) -> Self
    where
        IntoMethod: TryInto<Method> + Clone,
        <IntoMethod as TryInto<Method>>::Error: Debug,
        R: TryInto<RouteSpec>,
        R::Error: Debug,
    {
        let methods = methods
            .iter()
            .cloned()
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
    async fn run(&self, mut conn: Conn) -> Conn {
        let method = conn.method();
        let original_captures = conn.take_state();
        let path = conn.path();
        let mut has_path = false;

        if let Some(m) = self.best_match(conn.method(), path) {
            let mut captures = m.captures().into_owned();

            let route = m.route().clone();

            if let Some(CapturesNewType(mut original_captures)) = original_captures {
                original_captures.append(captures);
                captures = original_captures;
            }

            log::debug!("running {}: {}", m.route(), m.1.name());
            let mut new_conn = m
                .handler()
                .1
                .run({
                    if let Some(wildcard) = captures.wildcard() {
                        conn.push_path(String::from(wildcard));
                        has_path = true;
                    }

                    conn.with_state(CapturesNewType(captures))
                        .with_state(RouteSpecNewType(route))
                })
                .await;

            if has_path {
                new_conn.pop_path();
            }
            new_conn
        } else if method == Method::Options && self.handle_options {
            let allow = self
                .routefinder
                .methods_matching(path)
                .iter()
                .map(|m| m.as_ref())
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
        let path = conn.path();
        if let Some(m) = self.best_match(conn.method(), path) {
            m.handler().1.before_send(conn).await
        } else {
            conn
        }
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        if let Some(m) = self.best_match(*upgrade.method(), upgrade.path()) {
            m.1.has_upgrade(upgrade)
        } else {
            false
        }
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        self.best_match(*upgrade.method(), upgrade.path())
            .unwrap()
            .handler()
            .1
            .upgrade(upgrade)
            .await
    }

    fn name(&self) -> std::borrow::Cow<'static, str> {
        "Router".into()
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
        let routefinder = mem::take(&mut self.routefinder);
        for (route, (methods, mut handler)) in routefinder.0 {
            handler.init(info).await;
            self.routefinder.add(methods, route, handler);
        }
    }
}

impl Debug for Router {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("Router ")?;
        let mut set = f.debug_set();

        for (route, (methods, handler)) in &self.routefinder.0 {
            set.entry(&format_args!("{} {} -> {}", methods, route, handler.name()));
        }
        set.finish()
    }
}
