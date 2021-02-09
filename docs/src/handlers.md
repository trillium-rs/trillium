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

Here's a simple example of an application that responds to a request like http://localhost:8000/greet/earth with "hello earth" and http://localhost:8000/greet/mars with "hello mars" and responds to http://localhost:8000 with "hello everyone"

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

        conn.with_cookie(Cookie::build("some-cookie-name", "some-cookie-value").path("/").finish())
    }
];

```

## Sessions

## Logger

## Web Socket support

## Static file serving

## Template engines


