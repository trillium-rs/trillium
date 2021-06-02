# Check out the documentation for more info


* [📖 guide docs 📖](https://trillium.rs/)
* [📑 rustdocs for main 📑](https://docs.trillium.rs)


# Just show me some code!

[here's a kitchen-sink example
app](https://github.com/trillium-rs/trillium/blob/main/example/src/main.rs). It
makes use of the smol-based server, the router, cookies, sessions,
websockets, the devlogger, and the static file server, but it doesn't
really make any cohesive sense yet

[here's a similar example that can be deployed as an aws
lambda](https://github.com/trillium-rs/trillium/blob/main/aws-lambda-example/src/main.rs). The
actual content is very similar to the kitchen-sink example app, and
that's the point. With few exceptions (websockets, local asset
deployment), a trillium app that runs directly will also run in aws
lambda.


## Other examples and links:

* trillium
  * [rustdocs (main)](https://docs.trillium.rs/trillium/index.html)
* trillium-http
  > a lower abstraction than trillium itself, but potentially usable directly for some use cases
  * [rustdocs (main)](https://docs.trillium.rs/trillium_http/index.html)
  * [example](https://github.com/trillium-rs/trillium/blob/main/http/examples/http.rs)
  * [tokio example](https://github.com/trillium-rs/trillium/blob/main/http/examples/tokio-http.rs)
* file serving
  * static
    > serves static assets from the file system
    * [example](https://github.com/trillium-rs/trillium/blob/main/static/examples/static.rs)
    * [rustdocs (main)](https://docs.trillium.rs/trillium_static/index.html)
  * static-compiled
    > includes assets in the compiled binary
    * [example](https://github.com/trillium-rs/trillium/blob/main/static-compiled/examples/static-compiled.rs) 
    * [rustdocs (main)](https://docs.trillium.rs/trillium_static_compiled/index.html)
* templating
  * askama
    * [rustdocs (main)](https://docs.trillium.rs/trillium_askama/index.html)
    * [example](https://github.com/trillium-rs/trillium/blob/main/askama/examples/askama.rs)
  * tera
    * [rustdocs (main)](https://docs.trillium.rs/trillium_tera/index.html)
    * [example](https://github.com/trillium-rs/trillium/blob/main/tera/examples/tera.rs)
  * handlebars
    * [rustdocs (main)](https://docs.trillium.rs/trillium_handlebars/index.html)
    * [example](https://github.com/trillium-rs/trillium/blob/main/handlebars/examples/handlebars.rs)
* http client
  > yes, trillium has its own http client. this is primarily for the reverse proxy, but has connection pooling and other nice features that should make it fairly general-purpose.
  * [ ] <span style="color:red">rustdocs (main)</span>
  * [example](https://github.com/trillium-rs/trillium/blob/main/client/examples/client.rs)
* websockets
  * [rustdocs (main)](https://docs.trillium.rs/trillium_websockets/index.html)
  * [example](https://github.com/trillium-rs/trillium/blob/main/websockets/examples/websockets.rs)
* cookies
  * [rustdocs (main)](https://docs.trillium.rs/trillium_cookies/index.html)
  * [example](https://github.com/trillium-rs/trillium/blob/main/cookies/examples/cookies.rs)
* reverse proxy
  > this reverse proxy still has some work to go before being used in production but already supports things like forwarding arbitrary http protocol upgrades such as websockets
  * [ ] <span style="color:red">rustdocs (main)</span>
  * [example](https://github.com/trillium-rs/trillium/blob/main/proxy/examples/proxy.rs)
* sessions
  * [rustdocs (main)](https://docs.trillium.rs/trillium_sessions/index.html)
  * [example](https://github.com/trillium-rs/trillium/blob/main/sessions/examples/sessions.rs)
* logger
  * [ ] <span style="color:red">rustdocs (main)</span>
  * [example](https://github.com/trillium-rs/trillium/blob/main/logger/examples/logger.rs)
* Router
  * [rustdocs (main)](https://docs.trillium.rs/trillium_router/index.html)
  * [example](https://github.com/trillium-rs/trillium/blob/main/router/examples/router.rs)
  * [nested-router example](https://github.com/trillium-rs/trillium/blob/main/router/examples/nested-router.rs)
  * [router-with-macro example](https://github.com/trillium-rs/trillium/blob/main/router/examples/router-with-macro.rs)
* tls
  * rustls
    * [ ] <span style="color:red">rustdocs (main)</span>
    * [example](https://github.com/trillium-rs/trillium/blob/main/rustls/examples/rustls.rs)
  * native-tls
    * [ ] <span style="color:red">rustdocs (main)</span>
    * [example](https://github.com/trillium-rs/trillium/blob/main/native-tls/examples/native-tls.rs)
* servers
  * smol
    * [rustdocs (main)](https://docs.trillium.rs/trillium_smol/index.html)
    * [example](https://github.com/trillium-rs/trillium/blob/main/smol/examples/smol.rs)
    * [example-with-config](https://github.com/trillium-rs/trillium/blob/main/smol/examples/smol-with-config.rs)
  * aws-lambda
    * [rustdocs (main)](https://docs.trillium.rs/trillium_aws_lambda/index.html)
    * [example](https://github.com/trillium-rs/trillium/blob/main/aws-lambda-server/examples/aws-lambda.rs)
  * tokio
    * [rustdocs (main)](https://docs.trillium.rs/trillium_tokio/index.html)
    * [example](https://github.com/trillium-rs/trillium/blob/main/tokio/examples/tokio.rs)
  * async-std
    * [rustdocs (main)](https://docs.trillium.rs/trillium_async_std/index.html)
    * [example](https://github.com/trillium-rs/trillium/blob/main/async-std/examples/async-std.rs)
* html-rewriter based on cloudflare's lol-html
  * [ ] <span style="color:red">rustdocs (main)</span>
  * [html-rewriter](https://github.com/trillium-rs/trillium/blob/main/html-rewriter/examples/html-rewriter.rs) example
  > I'm not certain if I'm going to publish this as part of trillium, as it is !Send and that requires some more design for cross-runtime compat. It's fun that it works for reverse proxies, though.
