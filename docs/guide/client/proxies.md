---
title: Outbound proxies
---

# Outbound proxies

A client can send requests through an HTTP proxy. Ordinary HTTP requests are forwarded to the proxy as absolute-form requests (`GET http://origin.example/path`); HTTPS requests use CONNECT, then negotiate TLS with the origin through the tunnel. HTTP requests that need the origin's own connection, such as WebSocket upgrades and HTTP/2 prior knowledge (h2c), also use CONNECT. The request URL and `Host` still identify the origin.

Proxy routing is opt-in. Constructing a client does not read proxy environment variables, and installing a proxy does not change certificate trust.

## A proxy

```rust
# [dependencies]
# trillium-client = { path = "../client" }
# trillium-smol = { path = "../smol" }
#
use trillium_client::{Client, Proxy};
use trillium_smol::ClientConfig;

fn client() -> std::io::Result<Client> {
    let proxy = Proxy::new("http://proxy.example:8080")?
        .except(["localhost", "127.0.0.0/8", "[::1]", ".internal.example"])?;
    Ok(Client::new(ClientConfig::default()).with_proxy(proxy))
}
# fn main() {}
```

A `Proxy` carries both HTTP and HTTPS origins. `with_schemes(ProxySchemes::Http)` or `with_schemes(ProxySchemes::Https)` narrows it to one, and requests for the other scheme connect directly. The endpoint may be `http://` or `https://`; without a scheme, `http://` is assumed.

`except` adds bypass entries, which are matched against the request URL's host without DNS lookups:

- `example.com`, `.example.com`, and `*.example.com` match the domain and its subdomains, not `notexample.com`.
- IP addresses and CIDR networks match IP-literal URLs, not addresses obtained by resolving a domain.
- An optional port limits a match to that effective URL port. Use brackets for an IPv6 literal with a port, such as `[::1]:8080`.
- Each entry is one pattern, written exactly: an empty entry, surrounding whitespace, or a comma is an error, as is any malformed entry.

For HTTPS origins, use a TLS-capable connector as usual, such as `trillium_rustls::RustlsConfig`. The TLS server name and certificate identity are the origin's name or IP address, not the proxy's. An intercepting proxy therefore needs to present a certificate trusted by that connector. Configure any additional trust explicitly through the TLS adapter; proxy configuration never installs roots or reads certificate environment variables.

An `https://` proxy endpoint needs a TLS-capable connector too. The same connector validates the proxy's certificate on the connection to the proxy, then the origin's certificate inside the tunnel, so both are checked against the same trust roots: trusting a private certificate authority for a proxy also trusts it for origins.

## Routing

`with_proxy` takes any `ProxyRouter`, which chooses a proxy for each request from its original URL, or none to connect directly. A `Proxy` is one router. An array or `Vec` of routers is another: the first one that returns a proxy wins, so a later entry can catch what an earlier one exempts.

```rust
# [dependencies]
# trillium-client = { path = "../client" }
#
use trillium_client::Proxy;

fn proxies() -> std::io::Result<[Proxy; 2]> {
    Ok([
        Proxy::new("http://corporate.example:8080")?.except([".partner.example"])?,
        Proxy::new("http://partner-gateway.example:3128")?,
    ])
}
# fn main() {}
```

A function from `&Url` to `Option<Proxy>` is a router too. Annotate the closure's argument type so it accepts any URL borrow:

```rust
# [dependencies]
# trillium-client = { path = "../client" }
#
use trillium_client::{Proxy, ProxyRouter, Url};

fn router() -> std::io::Result<impl ProxyRouter> {
    let corporate = Proxy::new("http://proxy.corp.example:8080")?;
    Ok(move |url: &Url| {
        url.host_str()?
            .ends_with(".corp.example")
            .then(|| corporate.clone())
    })
}
# fn main() {}
```

A proxy that a router returns still applies its own schemes and bypass entries. `Option<R>` routes nothing when it is `None`. A router runs once per request, including each redirect, not again for protocol negotiation or pool lookup.

## Environment configuration

```rust
# [dependencies]
# trillium-client = { path = "../client" }
# trillium-smol = { path = "../smol" }
#
use trillium_client::{Client, Proxy};
use trillium_smol::ClientConfig;

fn client() -> std::io::Result<Client> {
    Ok(Client::new(ClientConfig::default()).with_proxy(Proxy::from_env()?))
}
# fn main() {}
```

`Proxy::from_env()` takes a snapshot of these variables:

| Variable | Meaning |
| --- | --- |
| `http_proxy` / `HTTP_PROXY` | Proxy for HTTP origins |
| `https_proxy` / `HTTPS_PROXY` | Proxy for HTTPS origins |
| `all_proxy` / `ALL_PROXY` | Fallback when the origin's scheme has no nonempty setting |
| `no_proxy` / `NO_PROXY` | Comma-separated bypass list; whitespace around entries and empty entries are ignored |

It returns the list you could write yourself: a `Proxy` for each configured endpoint, narrowed to the scheme it serves (or carrying both when only `all_proxy` is set), each with the `no_proxy` entries added through `except`. When nothing is configured the list is empty, and every request connects directly. A `no_proxy` entry of `*` also produces an empty list, which turns off proxy variables inherited from a parent environment without unsetting them.

Lowercase takes precedence over uppercase, even when empty. Uppercase `HTTP_PROXY` is ignored when `REQUEST_METHOD` is set: CGI, which always sets `REQUEST_METHOD`, passes an incoming request's `Proxy` header to the program as `HTTP_PROXY`, so honoring it there would let any client choose the proxy. Later changes to the environment do not change an existing list. Invalid settings return an error, naming the variable, instead of silently selecting a direct route. Ordinary client construction remains independent of all these variables.

## Authentication

URL user information supplies Basic proxy authorization: `Proxy::new("http://user:pass@proxy.example:8080")` sends `Proxy-Authorization: Basic …`, with percent escapes decoded before encoding, and removes the credentials from the stored endpoint. For another scheme, set the header directly:

```rust
# [dependencies]
# trillium-client = { path = "../client" }
#
use trillium_client::{KnownHeaderName, Proxy};

fn proxy() -> std::io::Result<Proxy> {
    Ok(Proxy::new("http://proxy.example:8080")?
        .with_connect_header(KnownHeaderName::ProxyAuthorization, "Bearer token"))
}
# fn main() {}
```

The client sends `Proxy-Authorization` to the proxy for forward requests and CONNECT, never to a tunneled origin or a bypassed destination. Other CONNECT headers are sent only with CONNECT. Proxy credentials come only from the router's proxies: while a router is installed, a `Proxy-Authorization` header set on an individual request is removed, including on requests sent directly. Basic authorization is not encrypted on a plaintext connection to the proxy.

Proxy authentication is preemptive. A proxy that rejects the credentials answers `407 Proxy Authentication Required`: a forwarded request receives that response like any other, and a refused CONNECT fails the request (see below). The client does not retry with a challenge-response scheme.

## Connections and protocol limits

A failed proxy connection or rejected CONNECT fails the request. It never triggers a direct-origin fallback. A non-2xx CONNECT response is returned inside `Error::Io` with `ErrorKind::ConnectionRefused`, and its inner error downcasts to `ProxyConnectError`:

```rust
# [dependencies]
# trillium-client = { path = "../client" }
#
use trillium_client::{Error, ProxyConnectError};

/// The proxy's refusal to open a tunnel, if that is why the request failed.
fn proxy_refusal(error: &Error) -> Option<&ProxyConnectError> {
    match error {
        Error::Io(io) => io.get_ref()?.downcast_ref(),
        _ => None,
    }
}
# fn main() {}
```

`ProxyConnectError::status` is the proxy's response status and `headers` its response headers, such as `Proxy-Authenticate` on a 407. A code that `Status` doesn't represent is reported as the `x00` status of its class, so a `520` is `500 Internal Server Error`.

The origin's name is sent to the proxy without a local origin DNS lookup; the proxy resolves it. The proxy's own host name is resolved by the connector, including on a client configured for encrypted DNS; give the proxy endpoint as an IP address to avoid any system DNS lookup. Bypassed requests use the client's ordinary resolution path. For a proxied request, `Conn::peer_addr` is the proxy's address.

Connections are pooled by origin and proxy, including the proxy's CONNECT headers, so requests routed to proxies with different credentials never share a connection. Calling `without_proxy()` returns a client that connects directly for every request.

Forward requests and the CONNECT hop to the proxy use HTTP/1.1. HTTPS tunnels can negotiate HTTP/1.1 or HTTP/2 with the origin. Plaintext requests also use CONNECT tunnels when they need the origin's own connection: WebSocket upgrades, explicit HTTP/2 (h2c prior knowledge), and caller-built CONNECT requests. For a caller-built CONNECT, the outer tunnel reaches the URL's origin, and the caller's CONNECT request is sent inside it. Some proxies only allow CONNECT to port 443, so they may reject tunnels to other ports.

An HTTP/3 hint on a proxied request continues over HTTP/1.1 or HTTP/2 through the proxy; it never opens a direct QUIC connection. WebTransport, which runs only over HTTP/3, fails with `webtransport::ErrorKind::Http3Unavailable` when a proxy is selected.
