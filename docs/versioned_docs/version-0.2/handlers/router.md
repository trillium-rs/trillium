# Router

[rustdocs (main)](https://docs.trillium.rs/trillium_router/index.html)

The trillium router is based on
[routefinder](https://github.com/jbr/routefinder). This router
supports two types of patterns: Untyped params and a single
wildcard. Named params are captured in a map-like interface. Any
handler can be mounted inside of a Router (including other Routers),
allowing entire applications to be mounted on a path, and allowing for
tuple handlers to be run on a given route. Any handler mounted inside
of a route that includes a `*` will have the url rewritten to the
contents of that star.

Alternative routers that are not based on routefinder are a prime
opportunity for innovation and exploration.

Here's a simple example of an application that responds to a request
like http://localhost:8000/greet/earth with "hello earth" and
http://localhost:8000/greet/mars with "hello mars" and responds to
http://localhost:8000 with "hello everyone"

```rust
use trillium::Conn;
use trillium_router::{Router, RouterConnExt};

pub fn main() {
    trillium_smol::run(
        Router::new()
            .get("/", |conn: Conn| async move { conn.ok("hello everyone") })
            .get("/hello/:planet", |conn: Conn| async move {
                let planet = conn.param("planet").unwrap();
                let response_body = format!("hello {planet}");
                conn.ok(response_body)
            }),
    );
}
```


## Nesting

Trillium also supports nesting routers, making it possible to express
complex sub-applications, vaguely along the lines of a [rails
engine](https://guides.rubyonrails.org/engines.html). When there are
additional types of routers, it will be possible for an application
built with one type of router to be published as a crate and nested
inside of another router as long as they depend on a compatible
version of the `trillium` crate.

```rust
use trillium::{conn_try, conn_unwrap, Conn, Handler};
use trillium_logger::Logger;
use trillium_router::{Router, RouterConnExt};

struct User {
    id: usize,
}

mod nested_app {
    use super::*;
    async fn load_user(conn: Conn) -> Conn {
        let id = conn_try!(conn.param("user_id").unwrap().parse(), conn);
        let user = User { id }; // imagine we were loading a user from a database here
        conn.with_state(user)
    }

    async fn greeting(mut conn: Conn) -> Conn {
        let user = conn_unwrap!(conn.take_state::<User>(), conn);
        conn.ok(format!("hello user {}", user.id))
    }

    async fn post(mut conn: Conn) -> Conn {
        let user = conn_unwrap!(conn.take_state::<User>(), conn);
        let body = conn_try!(conn.request_body_string().await, conn);
        conn.ok(format!("hello user {}, {}", user.id, body))
    }

    async fn some_other_route(conn: Conn) -> Conn {
        conn.ok("this is an uninspired example")
    }

    pub fn handler() -> impl Handler {
        (
            load_user,
            Router::new()
                .get("/greeting", greeting)
                .get("/some/other/route", some_other_route)
                .post("/post", post),
        )
    }
}
pub fn main() {
    env_logger::init();
    trillium_smol::run((
        Logger::new(),
        Router::new()
            .get("/", |conn: Conn| async move { conn.ok("hello everyone") })
            .any(&["get", "post"], "/users/:user_id/*", nested_app::handler()),
    ));
}
```
