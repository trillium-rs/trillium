# Check out the documentation for more info


* [📖 guide docs 📖](https://trillium.rs/)
* [📑 rustdocs for main 📑](https://docs.trillium.rs)


# Just show me some code!

[here's a kitchen-sink example app](https://github.com/trillium-rs/trillium/blob/main/example/src/main.rs). It makes use of the smol-based server, the router, cookies, sessions, websockets, the devlogger, and the static file server, but it doesn't really make any cohesive sense yet

[here's a similar example that can be deployed as an aws lambda](https://github.com/trillium-rs/trillium/blob/main/aws-lambda-example/src/main.rs). The actual content is very similar to the kitchen-sink example app, and that's the point. With few exceptions, a trillium app that runs directly will also run in aws lambda.

## Other examples and links:
* trillium itself has no examples yet somehow
  * [rustdocs (main)](https://docs.trillium.rs/trillium/index.html)
* trillium-http, a lower abstraction than trillium itself, but potentially usable directly
  * [rustdocs (main)](https://docs.trillium.rs/trillium_http/index.html)
  * [http](https://github.com/trillium-rs/trillium/blob/main/http/examples/http.rs) example
  * [tokio-http](https://github.com/trillium-rs/trillium/blob/main/http/examples/tokio-http.rs) example using trillium-http on tokio
* [websockets](https://github.com/trillium-rs/trillium/blob/main/websockets/examples/websockets.rs) example
* templating
  * [askama](https://github.com/trillium-rs/trillium/blob/main/askama/examples/askama.rs) example
  * [tera](https://github.com/trillium-rs/trillium/blob/main/tera/examples/tera.rs) example
  * [handlebars](https://github.com/trillium-rs/trillium/blob/main/handlebars/examples/handlebars.rs) example
* http client
  * [client](https://github.com/trillium-rs/trillium/blob/main/client/examples/client.rs) example
  > yes, trillium has its own http client. this is primarily for the reverse proxy, but has connection pooling and other nice features that should make it fairly general-purpose.
* [cookies](https://github.com/trillium-rs/trillium/blob/main/cookies/examples/cookies.rs) example
* [html-rewriter](https://github.com/trillium-rs/trillium/blob/main/html-rewriter/examples/html-rewriter.rs) example
* [logger](https://github.com/trillium-rs/trillium/blob/main/logger/examples/logger.rs) example
* Router
  * [nested-router](https://github.com/trillium-rs/trillium/blob/main/router/examples/nested-router.rs) example
  * [router](https://github.com/trillium-rs/trillium/blob/main/router/examples/router.rs) example
  * [router_with_macro](https://github.com/trillium-rs/trillium/blob/main/router/examples/router-with-macro.rs) example
* [Reverse proxy](https://github.com/trillium-rs/trillium/blob/main/proxy/examples/proxy.rs) example
* [sessions](https://github.com/trillium-rs/trillium/blob/main/sessions/examples/sessions.rs) example
* tls
  * [rustls](https://github.com/trillium-rs/trillium/blob/main/rustls/examples/rustls.rs) example
  * [native-tls](https://github.com/trillium-rs/trillium/blob/main/native-tls/examples/native-tls.rs) example
* servers -- these examples are quite boring, but are the entrypoint to all trillium apps
  * [smol-server](https://github.com/trillium-rs/trillium/blob/main/smol-server/examples/smol-server.rs) example
  * [smol-server-with-config](https://github.com/trillium-rs/trillium/blob/main/smol-server/examples/smol-server-with-config.rs) example
  * [aws-lambda](https://github.com/trillium-rs/trillium/blob/main/aws-lambda-server/examples/aws-lambda.rs) example
  * [tokio](https://github.com/trillium-rs/trillium/blob/main/tokio-server/examples/tokio.rs) example
  * [async-std-server](https://github.com/trillium-rs/trillium/blob/main/async-std-server/examples/async-std-server.rs) example
* file serving
  * [static](https://github.com/trillium-rs/trillium/blob/main/static/examples/static.rs) example -- serves assets from the file system
  * [static-compiled](https://github.com/trillium-rs/trillium/blob/main/static-compiled/examples/static-compiled.rs) example -- includes assets in the compiled binary

Any of the examples can be run by checking out this crate and executing `RUST_LOG=info cargo run --example {example name}`. By default, all examples will bind to port 8080, except `smol-server-with-config`.
