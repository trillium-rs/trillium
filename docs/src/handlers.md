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
  * use trillium as a reverse proxy for another server
  * [rustdocs (main)](https://docs.trillium.rs/trillium_proxy/index.html)
  * [example](https://github.com/trillium-rs/trillium/blob/main/proxy/examples/proxy.rs)
- method override
  * the trillium-method-override crate adds support for using post
    requests with a query param as a substitute for other http methods
  * [rustdocs (main)](https://docs.trillium.rs/trillium_method_override/index.html)
  * [example](https://github.com/trillium-rs/trillium/blob/main/method-override/examples/method-override.rs)
- head
  * the trillium-head crate supports responding to head requests
  * [rustdocs (main)](https://docs.trillium.rs/trillium_head/index.html)
  * [example](https://github.com/trillium-rs/trillium/blob/main/head/examples/head.rs)
- forwarding
  * the trillium-forwarding crate supports setting remote ip and
    protocol from forwarded/x-forwarded-* headers sent by trusted
    reverse proxies
  * [rustdocs (main)](https://docs.trillium.rs/trillium_forwarding/index.html)
  * [example](https://github.com/trillium-rs/trillium/blob/main/forwarding/examples/forwarding.rs)
