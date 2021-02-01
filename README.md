# Show me the code

[here's a kitchen-sink example app](https://github.com/rhizosphere/myco/blob/main/example/src/main.rs). It makes use of the smol-based server, the router, cookies, sessions, websockets, the devlogger, and the static file server.

# Names (Myco?? Grain??)

Rust was named after the [rust fungus](https://en.wikipedia.org/wiki/Rust_(fungus)) and mycology is the study of fungi. [Mycorrhizal networks](https://en.wikipedia.org/wiki/Mycorrhizal_network) are the OG [ent](https://en.wikipedia.org/wiki/Ent)ernet. Naming things is hard, this will probably change several times, as will the name of the gh organization.

In myco, a grain is both an endpoint and a middleware, the equivalent of a plug in [`plug`](https://github.com/elixir-plug/plug).

# Comparison with Tide

Myco should look pretty familiar to anyone who has used [tide](https://github.com/http-rs/tide) or plug. It is intended to be a hybrid of the best of both of these approaches. In places, it also is influenced by phoenix.

### `Fn(Conn) -> Conn` vs `Fn(Request) -> Response`, differences between Endpoints and Middleware

This is the biggest difference between tide and Myco. Myco moves the TcpStream into the Conn type, which represents both the request and the response, as well as a state typemap. In Myco, there is no distinction between endpoints and middleware -- they both are a `Grain` that provides several lifecycle hooks but is primarily any async `Fn(Conn) -> Conn`. A `Conn` also has a notion of being halted, and any `Grain` can halt the pipeline, preventing subsequent `Grains` from being executed.

### State lives as long as the http connection

Because tide's Requests are dropped in the endpoint, there is no way to move owned data from the request to the response.  As a consequence, any middleware that wants to put data into a request and get it back of the response out after the endpoint has run must put a cloned arc into the request state. With a Myco Conn, the extensions live throughout the request-response cycle, allowing it to directly own whatever data it needs, simplifying a lot of things.

### Batteries: Not included

Myco is built as a constellation of crates that can be composed. Instead of leaning on cargo features, Myco ships one crate for each feature set, and extends core types with extension traits.  This ensures that official Myco crates are not privileged in any way, and use the same interfaces that third party alternatives would. This also allows each crate to have its own independent release schedule.  Myco crates seek to have the smallest possible dependencies.

Related to the above, Myco ships with the bare minimum required to have interoperability between crates.  Everything else is a distinct crate. Some examples:

* State: In tide, setting global state is something that the tide Server handles. In Myco, this is achieved through a Grain.
* Router: In tide, the router exists outside of the middleware pipeline. In Myco, it is a distinct Grain that does not ship with Myco core.  This opens up the possibility for alternative routers that make different design tradeoffs. These routers would be equally interoperable with the rest of the Myco ecosystem.
* Cookies: Tide's cookies middleware ships in the tide crate. Myco provides this as a distinct crate.
* Sessions: Similarly, shipped as a distinct crate.
* Logger: Tide ships a default logger. Myco's loggers are provided externally.
* Static file server: Tide ships with serve_dir. Myco provides static file serving as a distinct crate.

If you do not use a feature, you should not have to compile it, nor do you need to update releases to that feature.  This should also make it easier to pick and choose which updates you want to install.

### `Futures` only, no build-in async runtime dep.

Myco's core has no direct dependency on any async runtime. Myco Servers currently exist for async-std, smol, and tokio (using async-compat).

### Error handling -- a different tradeoff than tide

Tide endpoints return a Result with an anyhow-like Error type, allowing the use of `?`. Myco requires always returning the `Conn` that was provided, and as a result does not support the use of `?`. If and when Myco interfaces return Errors, they will always be concrete errors. Until rust allows us to express `for<'a> Fn(&'a Something) -> impl Future<Output = Something> + Send + 'a`, Myco requires manual error handling.

### Http implementation

Although this may look similar to tide on the surface, it includes a different http implementation that only shares some code with [`async-h1`](https://github.com/http-rs/async-h1). This server-only http implementation can be used directly, and an example of that is [here](https://github.com/rhizosphere/myco/blob/main/http/examples/example.rs). One notable feature of this implementation is that the transport does not need to be `Clone` and is moved into the `Conn`.


# Contribution and community (this part only relevant once it's published)

This is going to run slightly different from other open source projects I've been part of. Please open an issue for discussion before opening a pull request or writing code for that pull request. PRs without prior discussion will be closed, every time.

This is a long term project and I expect contribution to be slow for a while. If you check it out in the early days, please come back in a few months.

Feature requests will be treated differently than bug reports. Since each of the crates is quite small, if a feature request expands the surface area of that crate substantially, we will often want to fork and create a new crate, either under your username or under this organization (whichever you prefer). Every crate in that is compatible with Myco will be featured prominently in the documentation, and the hope is that most of the crates that are currently in the workspace will be deprecated in preference to something better. The goal is not to build a monolith, but a community and ecosystem of interoperable components. If you want to write a myco crate and the core types don't provide the right extension point, that is the sort of issue that will be highest priority.

# List of crates:

## currently functional:
All crates listed here are believed to be fully functional and usable

### servers (you'll need one of these)

all of these will be supported out of the box by launch

|            | async-std | smol | tokio |
| ---------- | --------- | ---- | ----- |
| no tls     | ✅        | ✅   | ✅    |
| native tls | ❌        | ✅   | ❌    |
| rustls     | ❌        | ✅   | ❌    |

* [myco-tokio-server](https://github.com/rhizosphere/myco/tree/main/tokio-server)
* [myco-async-std-server](https://github.com/rhizosphere/myco/tree/main/async-std-server)
* [myco-smol-server](https://github.com/rhizosphere/myco/tree/main/smol-server)
* [myco-rustls](https://github.com/rhizosphere/myco/tree/main/rustls) --- currently just smol
* [myco-openssl](https://github.com/rhizosphere/myco/tree/main/openssl) --- currently just smol

### standard components
most app will want at least some of these, if not all. eventually these will be bundled in a "starter pack" type crate.

* [myco-cookies](https://github.com/rhizosphere/myco/tree/main/cookies)
* [myco-logger](https://github.com/rhizosphere/myco/tree/main/logger) - http logger
* [myco-router](https://github.com/rhizosphere/myco/tree/main/router) - the first router
* [myco-sessions](https://github.com/rhizosphere/myco/tree/main/sessions) - async-session backed sessions. requires cookies to be used as well
* [myco-static](https://github.com/rhizosphere/myco/tree/main/static) - serve static assets and directories
* [myco-websockets](https://github.com/rhizosphere/myco/tree/main/websockets)

### template engines

* [myco-askama](https://github.com/rhizosphere/myco/tree/main/askama) - compile-time templating
* [myco-tera](https://github.com/rhizosphere/myco/tree/main/tera) - run-time templating

### auth

* [myco-basic-auth](https://github.com/rhizosphere/myco/tree/main/basic-auth) - a very simple grain for fixed-username-and-password basic auth

## In the queue:
* json serialization/deserialization extensions
* sqlx transaction support
* standarized auth framework

## Pondering:
* types to decrease duplication across smol/tokio/async-std x rustls/openssl servers, like a trait for each of them to implement
