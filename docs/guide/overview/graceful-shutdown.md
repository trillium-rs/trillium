# Graceful shutdown

When a trillium server shuts down gracefully, it stops accepting new connections and lets in-flight requests finish before the process exits, rather than dropping them mid-response. On Unix systems this is wired up for you: `config().run()` installs handlers for `SIGINT` and `SIGTERM`, and the first such signal begins a graceful drain.

The mechanism behind it is the [`Swansong`](https://docs.rs/swansong) — a shared shutdown handle that long-lived work can observe, so a streaming response can end cleanly instead of being cut off when the server stops.

Most applications need nothing beyond the default. The rest of this page is for two cases: driving shutdown yourself, and running more than one server in a single process.

## Triggering shutdown yourself

If your application already handles signals — a larger service with its own lifecycle, or a process that starts and stops the server on demand — turn off trillium's built-in signal handling with `.without_signals()` and drive shutdown through the server's handle instead.

`spawn` returns a [`ServerHandle`](https://docs.trillium.rs/trillium_server_common/struct.ServerHandle.html). `shut_down()` begins a graceful drain and returns a future that completes once every in-flight request has finished; `block()` parks the current thread until the server has stopped.

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
#
use trillium::Conn;

fn main() {
    let handle = trillium_smol::config()
        .without_signals()
        .spawn(|conn: Conn| async move { conn.ok("hello") });

    // Hand `handle` to the rest of your application. When it's time to stop,
    // begin draining and wait for in-flight requests to finish:
    handle.shut_down().block();
}
```

You can also take the [`Swansong`](https://docs.rs/swansong) out of the handle with `handle.swansong()`, or build one yourself and pass it in with `.with_swansong(...)`. That shared handle is what lets several servers shut down together.

## Running several servers in one process

A single process can run more than one trillium server, each serving its own application on its own listener. This is the right tool when your ports do *different* things — a small responder on a plaintext port that redirects callers to HTTPS, an administrative or metrics endpoint kept off your public port, or two unrelated APIs co-located in one binary.

Give each server its own `config()` and a clone of a shared [`Swansong`](https://docs.rs/swansong). Exactly one server installs the operating-system signal handlers; the rest opt out with `.without_signals()`. They all share the swansong, so the shutdown signal that one server catches drains every server together.

How you keep the process running is up to you: `spawn` launches a server in the background and returns immediately, while `run` blocks until shutdown. Below, the ride-along server is spawned and the main one blocks on `run`, but spawning both and awaiting the swansong works just as well — all that matters is that one server is handling signals and that the process stays alive.

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-redirect = { path = "../redirect" }
#
use trillium::Conn;
use trillium_redirect::RedirectConnExt;
use trillium_smol::Swansong;

fn main() {
    let swansong = Swansong::new();

    // A ride-along server: it shares the swansong but installs no signal
    // handlers, so it drains when the signal-handling server below does.
    trillium_smol::config()
        .with_port(8080)
        .with_swansong(swansong.clone())
        .without_signals()
        .spawn(|conn: Conn| async move { conn.redirect("https://example.com") });

    // The signal-handling server. On shutdown the shared swansong drains both.
    trillium_smol::config()
        .with_port(8443)
        .with_swansong(swansong)
        .run(|conn: Conn| async move { conn.ok("the app") });
}
```

If these servers need to share application state, build it once and clone it into each — they are independent servers that happen to live in the same process and shut down as one.

These are separate applications. When the *same* application needs to be reachable on more than one socket, bind several listeners on a single server instead — see [Listeners](./listeners.md).
