# Check out the documentation for more info


* [📖 guide docs 📖](https://trillium.rs/)
* [📑 rustdocs for main 📑](https://docs.trillium.rs)


# Just show me some code!

[here's a kitchen-sink example app](https://github.com/trillium-rs/trillium/blob/main/example/src/main.rs). It makes use of the smol-based server, the router, cookies, sessions, websockets, the devlogger, and the static file server, but it doesn't really make any cohesive sense yet

[here's a similar example that can be deployed as an aws lambda](https://github.com/trillium-rs/trillium/blob/main/aws-lambda-example/src/main.rs). The actual content is very similar to the kitchen-sink example app, and that's the point. With few exceptions, a trillium app that runs directly will also run in aws lambda.

## Other examples and links:
* trillium
  * [rustdocs (main)](https://docs.trillium.rs/trillium/index.html)
* trillium-http
  > a lower abstraction than trillium itself, but potentially usable directly for some use cases
  * [rustdocs (main)](https://docs.trillium.rs/trillium_http/index.html)
  * [http](https://github.com/trillium-rs/trillium/blob/main/http/examples/http.rs) example
  * [tokio-http](https://github.com/trillium-rs/trillium/blob/main/http/examples/tokio-http.rs) example using trillium-http on tokio
* file serving
  * [static](https://github.com/trillium-rs/trillium/blob/main/static/examples/static.rs) example -- serves assets from the file system
  * [static-compiled](https://github.com/trillium-rs/trillium/blob/main/static-compiled/examples/static-compiled.rs) example -- includes assets in the compiled binary
* templating
  * [askama](https://github.com/trillium-rs/trillium/blob/main/askama/examples/askama.rs) example
  * [tera](https://github.com/trillium-rs/trillium/blob/main/tera/examples/tera.rs) example
  * [handlebars](https://github.com/trillium-rs/trillium/blob/main/handlebars/examples/handlebars.rs) example
* http client
  * [client](https://github.com/trillium-rs/trillium/blob/main/client/examples/client.rs) example
  > yes, trillium has its own http client. this is primarily for the reverse proxy, but has connection pooling and other nice features that should make it fairly general-purpose.
* [websockets](https://github.com/trillium-rs/trillium/blob/main/websockets/examples/websockets.rs) example
* [cookies](https://github.com/trillium-rs/trillium/blob/main/cookies/examples/cookies.rs) example
* reverse proxy
  * [proxy](https://github.com/trillium-rs/trillium/blob/main/proxy/examples/proxy.rs) example
  > this reverse proxy still has some work to go before being used in production but already supports things like forwarding arbitrary http protocol upgrades such as websockets
* [sessions](https://github.com/trillium-rs/trillium/blob/main/sessions/examples/sessions.rs) example
* [logger](https://github.com/trillium-rs/trillium/blob/main/logger/examples/logger.rs) example
* Router
  * [nested-router](https://github.com/trillium-rs/trillium/blob/main/router/examples/nested-router.rs) example
  * [router](https://github.com/trillium-rs/trillium/blob/main/router/examples/router.rs) example
  * [router-with-macro](https://github.com/trillium-rs/trillium/blob/main/router/examples/router-with-macro.rs) example
* tls
  * [rustls](https://github.com/trillium-rs/trillium/blob/main/rustls/examples/rustls.rs) example
  * [native-tls](https://github.com/trillium-rs/trillium/blob/main/native-tls/examples/native-tls.rs) example
* servers -- these examples are quite boring, but are the entrypoint to all trillium apps
  * [smol-server](https://github.com/trillium-rs/trillium/blob/main/smol-server/examples/smol-server.rs) example
  * [smol-server-with-config](https://github.com/trillium-rs/trillium/blob/main/smol-server/examples/smol-server-with-config.rs) example
  * [aws-lambda](https://github.com/trillium-rs/trillium/blob/main/aws-lambda-server/examples/aws-lambda.rs) example
  * [tokio](https://github.com/trillium-rs/trillium/blob/main/tokio-server/examples/tokio.rs) example
  * [async-std-server](https://github.com/trillium-rs/trillium/blob/main/async-std-server/examples/async-std-server.rs) example
* html-rewriter based on cloudflare's lol-html
  * [html-rewriter](https://github.com/trillium-rs/trillium/blob/main/html-rewriter/examples/html-rewriter.rs) example
  > I'm not certain if I'm going to publish this as part of trillium, as it is !Send and that requires some more design for cross-runtime compat. It's fun that it works for reverse proxies, though.

Any of the examples can be run by checking out this crate and executing `RUST_LOG=info cargo run --example {example name}`. By default, all examples will bind to port 8080, except `smol-server-with-config`.


## Bugs! They certainly exist!
There are no _known_ bugs, but I have high confidence that there are many bugs. They'll be fixed promptly when we find them.

Current areas of suspected bugs:
* `trillium_http`
  * http/1.0 vs http/1.1. It does support both, but likely does not do so correctly yet. This just needs a bit more careful attention, and has been less important than showing the other capabilities of the trillium approach
  * trillium_http _may_ have security vulnerabilities, but once people are using trillium, I intend to make the http implementation comparable in correctness and speed to other rust http implementations

## Plans/aspirations:
* The router does not support "any method" routes yet.
* Querystring support with [querystrong](https://github.com/jbr/querystrong)
* I'd like to drop the dependency on http-types and evaluate the `http` crate. Right now the most important types trillium uses from http-types are Headers, Extensions, StatusCode, Method, and Body. Extensions probably could be replaced with [typemap](https://docs.rs/typemap/0.3.3/typemap/) and Body probably could be something simpler than http-types's body. The other types have direct analogues in `http`.
* I'm going to explore making Conn generic over the server type, which would open up the possibility of spawning tasks off of Conn. I might hate this in practice because it will require _application code_ import something like `trillium_smol_server::Conn` instead of `trillium::Conn`, but library code would be generic over `trillium::Conn<S> where S: Server`. One exciting upside of this is that some library could say `impl Handler<Smol> for MyHandler` instead of `impl<S> Handler<S> for MyHandler where S: Server`, allowing them to write different implementations.
* Logic for when the server is overwhelmed, like backpressure or load shedding. Potentially since we have a count of the number of open connections, we can also set a maximum number of connections and choices for what to do when that threshold is met (put them in a queue or just respond immediately with an error). This is not essential now, but I think it's valuable to be able to point to mature features like this when communicating with people assessing this as a framework to build on.
* Document and publish the CLI so that people can use hot reloading.
* "framework user" stuff, like
  * an ecto-inspired datamapper built on top of sqlx, primarily focused on how awkward it is to work with partial changesets in rust
  * tools for building apis
  * something more like phoenix channels than the current websocket implementation. this should support falling back to long polling
* Once I have a sense of how people are using trillium, I'd like to look into h2 and h3. I really don't want to write my own implementation of these, but I'm not sure how hard it will be to fit existing implementations into the `conn` model.


