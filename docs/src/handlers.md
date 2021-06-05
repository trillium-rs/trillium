# A tour of some of the handlers that exist today

In order for trillium to be a usable web framework, we offer a number
of core utilities. However, it is my hope that alternative
implementations for at least some of these will exist in order to
explore the design space and accommodate different design constraints
and tradeoffs. Because not every application will need this
functionality, they are each released as distinct crates from the core
of trillium.

- [Logger](./handlers/logger.md)
- [Cookies](./handlers/cookies.md)
- [Sessions](./handlers/sessions.md)
- [Template Engines](./handlers/templates.md)
- [Proxy](./handlers/proxy.md)
- [Static File Serving](./handlers/static.md)
- [Websockets](./handlers/websockets.md)
- [Router](./handlers/router.md)
- http client
  * [rustdocs (main)](https://docs.trillium.rs/trillium_client/index.html)
  * [example](https://github.com/trillium-rs/trillium/blob/main/client/examples/client.rs)
- reverse proxy
  * [rustdocs (main)](https://docs.trillium.rs/trillium_proxy/index.html)
  * [example](https://github.com/trillium-rs/trillium/blob/main/proxy/examples/proxy.rs)
- html-rewriter based on cloudflare's lol-html
  * [rustdocs (main)](https://docs.trillium.rs/trillium_html_rewriter/index.html)
  * [html-rewriter](https://github.com/trillium-rs/trillium/blob/main/html-rewriter/examples/html-rewriter.rs) example
- trillium-http
  * [rustdocs (main)](https://docs.trillium.rs/trillium_http/index.html)
  * [example](https://github.com/trillium-rs/trillium/blob/main/http/examples/http.rs)
  * [tokio example](https://github.com/trillium-rs/trillium/blob/main/http/examples/tokio-http.rs)
