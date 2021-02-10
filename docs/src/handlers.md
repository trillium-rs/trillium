# A tour of some of the handlers that exist today

In order for myco to be a usable web framework, we offer a number of
core utilities. However, it is our hope that alternative
implementations for at least some of these will exist in order to
explore the design space and accommodate different design constraints
and tradeoffs. Because not every application will need this
functionality, they are each released as distinct libraries from the
core of myco.

## Routers

Currently there is only one router, based on the same interface as
tide's router,
[`route-recognizer`](https://github.com/http-rs/route-recognizer). This
router supports two types of patterns: Untyped params and wildcard
globs. Named params are captured in a map-like interface. Any handler
can be mounted inside of a Router (including other Routers), allowing
entire applications to be mounted on a path, and allowing for
sequences to be run on a given route.

Alternative routers that are not based on route-recognizer are a prime
opportunity for innovation and exploration.

Here's a simple example of an application that responds to a request
like http://localhost:8000/greet/earth with "hello earth" and
http://localhost:8000/greet/mars with "hello mars" and responds to
http://localhost:8000 with "hello everyone"

```rust
use myco_router::{Router, RouterConnExt};
run("localhost:8000", (),
    Router::new()
        .get("/", |conn: Conn| async move { conn.ok("hello everyone") })
        .get("/hello/:planet", |conn: Conn| async move {
            let planet = conn.param("planet").unwrap();
            let response_body = format!("hello {}", planet);
            conn.ok(response_body)
        })
);
```

### Experimental macro

Because routers are very common, there's also a macro. Currently, the above example with a macro looks like:

```rust
use myco_router::{routes, RouterConnExt};
run("localhost:8000", (),
    routes![
        get "/" |conn: Conn| async move { conn.ok("hello everyone") },
        get "/hello/:planet" |conn: Conn| async move {
            let planet = conn.param("planet").unwrap();
            let response_body = format!("hello {}", planet);
            conn.ok(response_body)
        }
    ]
);
```

> 🚧 Feedback and improvements on the syntax are very welcome.

## Cookies

```rust
use myco_cookies::{Cookies, CookiesConnExt};

let handler = sequence![
    myco_cookies::Cookies,
    |conn: Conn| async move {
        if let Some(cookie_value) = conn.cookies().get("some-cookie-name") {
            println!("current cookie value: {}", cookie_value);
        }

        conn.with_cookie(Cookie::new("some-cookie-name", "some-cookie-value"))
    }
];

```

## Sessions

Sessions are a common convention in web frameworks, allowing for a
safe and secure way to associate server-side data with a given http
client (browser). Myco's session storage is built on the
`async-session` crate, which allows us to share session stores with
tide. Currently, these session stores exist:

* MemoryStore (reexported as myco_sessions::MemoryStore) [^1]
* CookieStore (reexported as myco_sessions::CookieStore) [^1]
* PostgresSessionStore and SqliteSessionStore from [async-sqlx-session](https://github.com/jbr/async-sqlx-session)
* RedisSessionStore from [async-redis-session](https://github.com/jbr/async-redis-session)
* MongodbSessionStore from [async-mongodb-session](https://github.com/http-rs/async-mongodb-session)

[^1]: The memory store and cookie store should be avoided for use in
    production applications. The memory store will lose all session
    state on server process restart, and the cookie store makes
    different security tradeoffs than the database-backed stores. If
    possible, use a database.

> ❗The session handler _must_ be used in conjunction with the cookie
> handler, and it must run _after_ the cookie handler. This particular
> interaction is also present in other frameworks, and is due to the
> fact that regardless of which session store is used, sessions use a
> secure cookie as a unique identifier.

```rust
use myco_sessions::{MemoryStore, SessionConnExt, Sessions};

let handler = sequence![
    myco_cookies::Cookies,
    Sessions::new(MemoryStore::new(), b"01234567890123456789012345678901123"),
    |conn: Conn| async move {
        let count: usize = conn.session().get("count").unwrap_or_default();
        conn.with_session("count", count + 1)
            .ok(format!("count: {}", count))
    }
];
```


## Logger

Currently there's just a DevLogger, but soon there will be more loggers. Myco loggers use the `log` crate and emit one line at the info log level per http request/response pair. We like to use the `env_logger` crate, but any `log` backend will work equally well.

```rust
pub fn main() {
    env_logger::init();
    run("localhost:8000", (), sequence![
        myco_logger::DevLogger,
        |conn: Conn| async move { conn.ok("ok!") }
    ]);
}
```

## WebSocket support

WebSockets work a lot like tide's, since I recently wrote that interface as well. One difference in myco is that the websocket connection also contains some aspects of the original http request, such as request headers, the request path and method, and any state that has been accumulated by previous handlers in a sequence.

```rust
use myco_websockets::{Message, WebSocket, WebSocketConnection};
pub fn main() {
    run("localhost:8000", (), WebSocket::new(|mut websocket| async move {
        while let Some(Ok(Message::Text(input))) = websocket.next().await {
            websocket.send_string(format!("received your message: {}", &input)).await;
        }
    }));
}
```

## Static file serving

Myco offers very rudimentary static file serving for now. There is a lot of room for improvement. This example serves any file within ./public (relative to the current working directory) at the root url, so ./public/index.html would be served at http://localhost:8000/index.html .

There are a large number of additional features that this crate needs in order to be usable in a production environment.

```rust
pub fn main() {
    run("localhost:8000", (), myco_static::Static::new("/", "./public"))
}
```

## Template engines



### Askama

### Tera

### Handlebars
