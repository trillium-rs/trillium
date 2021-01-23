# Show me the code

[here's a tiny example app](https://github.com/rhizosphere/myco/blob/main/example/src/main.rs). It makes use of the smol-based server, the router, cookies, sessions, websockets, the devlogger, and the static file server.

# Names (Myco?? Grain??)

Rust was named after the [rust fungus](https://en.wikipedia.org/wiki/Rust_(fungus)) and mycology is the study of fungi. Plus fungi are cool and [mycorrhizal networks](https://en.wikipedia.org/wiki/Mycorrhizal_network) are the OG [ent](https://en.wikipedia.org/wiki/Ent)ernet.

In myco, a grain is both an endpoint and a middleware. In [`plug`](https://github.com/elixir-plug/plug), the primary interface is called a plug, and in myco, a grain is the equivalent.

# Comparison with Tide

Myco should look pretty familiar to anyone who has used [tide](https://github.com/http-rs/tide) or plug. It is intended to be a hybrid of the best of both of these approaches. In places, it also is influenced by phoenix.

### `Fn(Conn) -> Conn` vs `Fn(Request) -> Response`, differences between Endpoints and Middleware

This is the biggest difference between tide and Myco. Myco moves the TcpStream into the Conn type, which represents both the request and the response, as well as a state typemap. In Myco, there is no distinction between endpoints and middleware -- they both are a `Grain` that provides several lifecycle hooks but is primarily any async `Fn(Conn) -> Conn`. A `Conn` also has a notion of being halted, and any `Grain` can halt the pipeline, preventing subsequent `Grains` from being executed.

## State lives as long as the http connection

Because tide's Requests are dropped in the endpoint, there is no way to move owned data from the request to the response.  As a result, any middleware that wants to put data into a request and get it back of the response out after the endpoint has run must put a clone into the request state. With a Myco Conn, the extensions live throughout the request-response cycle, allowing it to own whatever data it needs.

## Batteries: Not included

Myco is built as a constellation of crates that can be composed. Instead of leaning on cargo features, Myco ships one crate for each feature set, and extends core types with extension traits.  This ensures that official Myco crates are not privileged in any way, and use the same interfaces that third party alternatives would. This also allows each crate to have its own independent release schedule.  Myco crates seek to have the smallest possible dependencies.

Related to the above, Myco ships with the bare minimum required to have interoperability between crates.  Everything else is a distinct crate. Some examples:

* State: In tide, setting global state is something that the tide Server handles. In Myco, this is achieved through a Grain.
* Router: In tide, the router exists outside of the middleware pipeline. In Myco, it is a distinct Grain that does not ship with Myco core.  This opens up the possibility for alternative routers that make different design tradeoffs. These routers would be equally interoperable with the rest of the Myco ecosystem.
* Cookies: Tide's cookies middleware ships in the tide crate. Myco provides this as a distinct crate.
* Sessions: Similarly, shipped as a distinct crate.
* Logger: Tide ships a default logger. Myco's loggers are provided externally.
* Static file server: Tide ships with serve_dir. Myco provides static file serving as a distinct crate.

If you do not use a feature, you should not have to compile it, nor do you need to update releases to that feature.  This should also make it easier to pick and choose which updates you want to install.

## Futures only, minimal dependencies

Myco has no direct dependency on async-std or tokio, and should be able to support a variety of async runtimes. Currently there is support for `async-std` and `smol`, but tokio support is planned.

## Error handling -- a different tradeoff than tide

Tide endpoints return a Result with an anyhow-like Error type, allowing the use of `?`. Myco requires always returning the `Conn` that was provided, and as a result does not support the use of `?`. If and when Myco interfaces return Errors, they will always be concrete errors. Until rust allows us to express `for<'a> Fn(&'a Something) -> impl Future<Output = Something> + Send + 'a`, Myco requires manual error handling.

## Http implementation

Although this may look similar to tide on the surface, it includes a different http implementation that only shares some code with [`async-h1`](https://github.com/http-rs/async-h1). This server-only http implementation can be used directly, and an example of that is [here](https://github.com/rhizosphere/myco/blob/main/http/examples/example.rs).
