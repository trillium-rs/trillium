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
{{#include ../../router/examples/router.rs}}
```

### Experimental macro

Because routers are very common, there's also a macro. Currently, the above example with a macro looks like:

```rust
{{#include ../../router/examples/router_with_macro.rs}}
```

> 🚧 Feedback and improvements on the syntax are very welcome.

## Cookies

```rust
{{#include ../../cookies/examples/cookies.rs}}
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
{{#include ../../sessions/examples/sessions.rs}}
```

## Logger

Currently there's just a DevLogger, but soon there will be more
loggers. Myco loggers use the `log` crate and emit one line at the
info log level per http request/response pair. We like to use the
`env_logger` crate, but any `log` backend will work equally well.

```rust
{{#include ../../logger/examples/logger.rs}}
```

## WebSocket support

WebSockets work a lot like tide's, since I recently wrote that
interface as well. One difference in myco is that the websocket
connection also contains some aspects of the original http request,
such as request headers, the request path and method, and any state
that has been accumulated by previous handlers in a sequence.

```rust
{{#include ../../websockets/examples/websockets.rs}}
```

## Static file serving

Myco offers very rudimentary static file serving for now. There is a
lot of room for improvement. This example serves ../docs/book/
(relative to the current working directory) at the root url, so
../docs/book/index.html would be served at
http://localhost:8000/index.html . There is not currently
default-index-serving functionality, but that will be added soon.

There are a large number of additional features that this crate needs
in order to be usable in a production environment.

```rust
{{#include ../../static/examples/static.rs}}
```

## Template engines

There are currently three template engines for myco. Although they are in no way mutually exclusive, most applications will want at most one of these.

### Askama

Askama is a jinja-based template engine that preprocesses templates at
compile time, resulting in efficient and type-safe templates that are
compiled into the application binary. We recommend this approach as a
default. Here's how it looks:

Given the following file in `(cargo root)/templates/examples/hello.html`,
```django
{{#include ../../askama/templates/examples/hello.html}}
```

```rust
{{#include ../../askama/examples/askama.rs}}
```

### Tera

Tera offers runtime templating. Myco's tera integration provides an interface very similar to `phoenix` or `rails`, with the notion of `assigns` being set on the conn prior to render.


Given the following file in the same directory as main.rs (examples in this case),
```django
{{#include ../../tera/examples/hello.html}}
```

```rust
{{#include ../../tera/examples/tera.rs}}
```

### Handlebars

Handlebars also offers runtime templating. Given the following file in `examples/templates/hello.hbs`,

```handlebars
{{#include ../../handlebars/examples/templates/hello.hbs}}
```

```rust
{{#include ../../handlebars/examples/handlebars.rs}}
```
