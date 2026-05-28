# Listeners

A *listener* is a socket your server accepts connections on — a TCP port, a Unix domain socket, or a
QUIC/UDP endpoint for HTTP/3. Most servers need exactly one, and trillium binds it for you from the
environment. This page covers that default, how to bind several listeners on a single server, and
what happens when a bind fails. To run several independent servers in one process, see [Graceful
shutdown](./graceful-shutdown.md).

## The default: one listener from the environment

`config()` builds a server that reads its address from the environment when you `run()` it,
following the [12-factor](https://12factor.net/config) convention:

- `PORT` — the TCP port to bind.
- `HOST` — the interface to bind to (default `localhost`).

On Unix systems, two more conventions apply to `HOST`:

- If it begins with `.`, `/`, or `~`, it's treated as a filesystem path and bound as a Unix domain
  socket instead of a TCP port.
- A separate `LISTEN_FD` variable hands the server an already-open socket inherited from a parent
  process. This is how socket-activation tools like [catflap](https://crates.io/crates/catflap) and
  [systemfd](https://github.com/mitsuhiko/systemfd) hold a port open across restarts while you
  develop.

To pin the host and port in code instead of reading them from the environment:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
#
pub fn main() {
    trillium_smol::config()
        .with_port(8080)
        .with_host("127.0.0.1")
        .run(|conn: trillium::Conn| async move { conn.ok("hello world") });
}
```

If the bind fails — the port is already in use, or the process lacks permission — `run()` panics and
the server exits. If you need to recover from failure to bind, use the
[Config::listeners](https://docs.trillium.rs/trillium_server_common/struct.Config.html#method.listeners)

## Binding several listeners on one server

Reach for this when a single application needs to be reachable on more than one socket. If
you need to serve identical content across several network interfaces — a public address and a
VPC-internal one, say — to run an HTTP/3 endpoint on a different UDP port than your TLS listener, or
to offer the same content over both cleartext and TLS, bind each listener explicitly with
`.listeners()`:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
#
use trillium::Conn;

fn main() -> std::io::Result<()> {
    trillium_smol::config()
        .listeners()
        .bind_tcp("203.0.113.10:8080")?
        .bind_tcp("10.0.0.10:8080")?
        .run(|conn: Conn| async move { conn.ok("served on both interfaces") });
    Ok(())
}
```

Unlike running [several servers](./graceful-shutdown.md), this is one application reached through
several sockets: a single handler, a single shared-state set, initialized exactly once. If your
listeners would run *different* code, you want several servers, not several listeners.

Each listener has its own binding method, named for what it needs:

- `bind_tcp(addr)` — a plaintext TCP listener.
- `bind_tls(addr, acceptor)` — a TLS listener; the acceptor carries the certificate (see
  [TLS](./tls.md)).
- `bind_quic(addr, quic_config)` — an HTTP/3 endpoint over QUIC (see [HTTP/3](./http3.md)).
- `bind_uds(path)` — a Unix domain socket.
- `bind_env()` — resolve `HOST`/`PORT`/`LISTEN_FD` from the environment, exactly as the
  single-listener default does.
- `bind_server(server)` — adopt a socket you bound yourself.

`addr` is flexible: a bare port (bound on all interfaces), a `SocketAddr` or `(ip, port)` tuple, or
a `host:port` string that's resolved when you bind.

Each connection carries the listener it arrived on as a
[`Listener`](https://docs.trillium.rs/trillium/struct.Listener.html) in its state, reachable with
`conn.state::<Listener>()`. It reports the address, port, TLS status, and kind the request came in
on. Use it to observe where a request came from.

## Handling a failed bind

The binding methods on ListenerConfig bind eagerly and return `std::io::Result`, so a failure to
claim an address is a value you can inspect rather than a panic. The `?` in the example above
propagates that error out of `main`; you could instead match on it and fall back to another port,
log and continue with the listeners that did bind, or report a specific message.
