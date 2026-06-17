# Encrypted DNS

By default the client resolves hostnames the way the rest of your system does: it hands each name to the operating system's resolver, which usually sends it as a plaintext UDP query to whatever resolver the network handed you — your ISP, the coffee-shop router, a corporate middlebox. Every name you look up is visible, in the clear, to anyone on that path, and answerable by anyone willing to spoof a reply.

With the `hickory` cargo feature, the client can instead route all of its DNS through an encrypted resolver of your choice. The query travels inside TLS, HTTPS, or QUIC to a resolver you pick, so the network between you and that resolver sees only an encrypted connection, not the names inside it. The feature is named for [hickory-dns](https://docs.rs/hickory-proto), which provides the wire-format encoding underneath.

It's off by default because it changes where your DNS goes — a decision worth making deliberately rather than inheriting. But it's worth turning on whenever you can: the privacy win is real, and it unlocks DNS-level HTTP/3 discovery (below) as a bonus.

## Two reasons to turn it on

**Privacy.** Plaintext DNS leaks your entire browsing pattern to the local network and to your resolver operator. Encrypting the query closes the on-path leak and lets you choose who answers — a resolver with a privacy policy you trust instead of whoever happens to run the network you're on.

**SVCB/HTTPS records and HTTP/3 discovery.** [SVCB and HTTPS records](https://www.rfc-editor.org/rfc/rfc9460) (RFC 9460) are a DNS record type that lets a domain publish connection parameters — most usefully `alpn=h3`, advertising HTTP/3 support directly in DNS. The encrypted-DNS path fetches these records alongside the address lookup, so an HTTP/3-capable client reaches an `alpn=h3` domain over HTTP/3 on the *first* request, with no [`Alt-Svc`](https://www.rfc-editor.org/rfc/rfc7838) round-trip — the usual mechanism, where the first request goes out over HTTP/1.1 or HTTP/2 and only a later one is upgraded after the server advertises `Alt-Svc: h3`. The system resolver path doesn't surface SVCB records; this one does. So even setting privacy aside, encrypted DNS buys you faster, earlier HTTP/3.

## Choosing a transport

There are four configurators, one per transport. A client routes DNS through at most one resolver; calling a second configurator replaces the first (and logs a warning).

| Method | Transport | RFC | Requires |
| --- | --- | --- | --- |
| `with_doh` | DNS-over-HTTPS | [8484](https://www.rfc-editor.org/rfc/rfc8484) | a TLS connector |
| `with_doh3` | DNS-over-HTTPS, pinned to HTTP/3 | [8484](https://www.rfc-editor.org/rfc/rfc8484) | an HTTP/3-capable client |
| `with_dot` | DNS-over-TLS | [7858](https://www.rfc-editor.org/rfc/rfc7858) | a TLS connector |
| `with_doq` | DNS-over-QUIC | [9250](https://www.rfc-editor.org/rfc/rfc9250) | an HTTP/3-capable client |

`with_doh` is the one to reach for if you're unsure. DoH lookups ride the client's own connection pool, so they reuse and multiplex over the same connection as the resolver's other traffic, and a DoH-capable resolver is the most widely deployed. `with_doh3` is the same thing pinned to HTTP/3 — useful for a resolver that serves DoH over HTTP/3 but doesn't advertise it via `Alt-Svc`, which would otherwise leave the resolver connection on HTTP/2. `with_dot` opens a dedicated TLS connection per lookup. `with_doq` rides a cached, multiplexed QUIC connection.

Each configurator accepts a bare host or IP (`"1.1.1.1"`) or a full URL. A bare value is expanded to that transport's conventional endpoint — `https://1.1.1.1/dns-query` for DoH, `https://1.1.1.1:853` for DoT and DoQ — so the short form is usually all you need.

## Public resolvers

Three well-known public resolvers, and which transports each speaks:

| Resolver | Address | DoH | DoH3 | DoT | DoQ |
| --- | --- | :-: | :-: | :-: | :-: |
| Cloudflare | `1.1.1.1` | ✓ | ✓ | ✓ | |
| Google | `8.8.8.8` | ✓ | ✓ | ✓ | |
| Quad9 | `9.9.9.9` | ✓ | ✓ | ✓ | ✓ |

For DoH, DoH3, and DoT, Cloudflare and Google are solid defaults. For DNS-over-QUIC, Quad9 is the one of the three that offers it.

## Examples

DNS-over-HTTPS through Cloudflare. The resolver is reached over HTTPS, so the client needs a TLS connector:

```rust
# [dependencies]
# trillium-smol = { path = "../smol" }
# trillium-client = { path = "../client", features = ["hickory"] }
# trillium-rustls = { path = "../rustls" }
#
# fn main() {
use trillium_client::Client;
use trillium_rustls::RustlsConfig;
use trillium_smol::ClientConfig;

let client = Client::new(RustlsConfig::<ClientConfig>::default())
    .with_doh("1.1.1.1");
# let _ = client;
# }
```

DNS-over-TLS through Google, same TLS requirement:

```rust
# [dependencies]
# trillium-smol = { path = "../smol" }
# trillium-client = { path = "../client", features = ["hickory"] }
# trillium-rustls = { path = "../rustls" }
#
# fn main() {
use trillium_client::Client;
use trillium_rustls::RustlsConfig;
use trillium_smol::ClientConfig;

let client = Client::new(RustlsConfig::<ClientConfig>::default())
    .with_dot("8.8.8.8");
# let _ = client;
# }
```

DNS-over-QUIC through Quad9. DoQ (like DoH3) needs an HTTP/3-capable client, built with `new_with_quic`:

```rust
# [dependencies]
# trillium-tokio = { path = "../tokio" }
# trillium-client = { path = "../client", features = ["hickory"] }
# trillium-quinn = { path = "../quinn", features = ["webpki-roots"] }
# trillium-rustls = { path = "../rustls" }
#
# fn main() {
use trillium_client::Client;
use trillium_quinn::ClientQuicConfig;
use trillium_rustls::RustlsConfig;
use trillium_tokio::ClientConfig;

let client = Client::new_with_quic(
    RustlsConfig::<ClientConfig>::default(),
    ClientQuicConfig::with_webpki_roots(),
)
.with_doq("9.9.9.9");
# let _ = client;
# }
```

## What to expect once it's on

**Resolution is fail-closed.** Once a resolver is configured, a lookup it can't answer fails the request rather than quietly falling back to the system resolver. That's the point: a name never leaks back to a plaintext local resolver because the encrypted one had a bad day. An unreachable resolver — or one that doesn't speak the configured transport, such as a DoT host addressed over DoQ — fails the lookup with a descriptive error rather than stalling.

**The resolver's own host is the one bootstrap exception.** A hostname resolver is itself resolved once, via the underlying connector, to open the connection to it. Give the resolver as an IP address (`"1.1.1.1"` rather than a hostname) to skip even that one plaintext lookup.

**IP-literal requests skip the resolver.** A request to `https://192.0.2.1/` connects directly — there's nothing to look up and no SVCB records exist for a bare address.

**One resolution, shared.** A single resolution is cached and shared across HTTP/1, HTTP/2, and HTTP/3, so the protocol a request ends up using doesn't trigger a second lookup.
