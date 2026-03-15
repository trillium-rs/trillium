# Proxy

[rustdocs](https://docs.trillium.rs/trillium_proxy)

The `trillium-proxy` crate provides both reverse proxy and forward proxy modes, built on `trillium-client`. Both modes support HTTP/3 on the inbound side (when the server is configured with `trillium-quinn`) and on the outbound side (when the client is initialized with `Client::new_with_quic`).

## Reverse proxy

A reverse proxy forwards incoming requests to a fixed upstream and streams the response back. Pass a `Client` and an upstream URL to `Proxy::new`:

```rust,noplaypen
{{#include ../../../proxy/examples/proxy.rs}}
```

### 404 pass-through

By default, a `404 Not Found` from the upstream is not forwarded — the conn is left unhalted so the next handler in the chain can respond. This makes it easy to proxy some paths while serving others locally:

```rust,noplaypen
trillium_smol::run((
    router()
        .get("/api/*", Proxy::new(client.clone(), "http://api-server/api/")),
    static_files,
));
```

Call `.proxy_not_found()` to forward 404 responses from the upstream as-is instead.

### Load balancing

When multiple upstream addresses are provided, the proxy distributes requests across them. The `RoundRobin` selector is built in; `ConnectionCounting` and `RandomSelector` are available via cargo features:

```rust,noplaypen
use trillium_proxy::{Proxy, upstream::{ConnectionCounting, IntoUpstreamSelector}};

let upstream = ["http://app1:8080", "http://app2:8080", "http://app3:8080"]
    .into_iter()
    .collect::<ConnectionCounting<_>>();

Proxy::new(Client::new(ClientConfig::default()), upstream)
```

You can also supply a closure as a custom upstream selector: `fn(&mut Conn) -> Option<Url>`.

### Via header

Add a `Via` header to forwarded requests to identify the proxy in the request chain:

```rust,noplaypen
Proxy::new(client, "http://upstream/").with_via_pseudonym("my-proxy")
```

### WebSocket proxying

WebSocket upgrade requests are not proxied by default. Enable with:

```rust,noplaypen
Proxy::new(client, "http://upstream/").with_websocket_upgrades()
```

### HTTP/3 upstream connections

To proxy over HTTP/3 to the upstream, initialize the client with `new_with_quic`. The proxy will upgrade connections automatically when the upstream advertises H3 support via `Alt-Svc`:

```rust,noplaypen
use trillium_client::Client;
use trillium_quinn::ClientQuicConfig;
use trillium_rustls::RustlsConfig;
use trillium_tokio::ClientConfig;

let client = Client::new_with_quic(
    RustlsConfig::<ClientConfig>::default(),
    ClientQuicConfig::with_webpki_roots(),
);

Proxy::new(client, "https://upstream/")
```

## Forward proxy

A forward proxy acts as an intermediary for clients making requests to arbitrary destinations. The client sends the request to the proxy, and the proxy forwards it onward.

Use `ForwardProxy` as the upstream selector to read the destination from the request itself, and add `ForwardProxyConnect` to handle `CONNECT` tunnel requests (needed for HTTPS through a forward proxy):

```rust,noplaypen
use trillium_client::Client;
use trillium_logger::logger;
use trillium_proxy::{ForwardProxyConnect, proxy, upstream::ForwardProxy};
use trillium_smol::ClientConfig;

fn main() {
    trillium_smol::run((
        logger(),
        ForwardProxyConnect::new(ClientConfig::default()),
        proxy(Client::new(ClientConfig::default()), ForwardProxy),
    ));
}
```

`ForwardProxyConnect` handles the `CONNECT` method by establishing a TCP tunnel between the client and the upstream host. Plain HTTP requests are handled by the `proxy(client, ForwardProxy)` handler, which reads the destination URL from the request's path-and-query.
