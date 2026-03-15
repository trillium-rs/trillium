# WebTransport

[rustdocs](https://docs.trillium.rs/trillium_webtransport)

WebTransport is a browser API and protocol built on HTTP/3 and QUIC. It gives each session:

- **Bidirectional streams** — both sides can read and write; streams are independent, so a slow one doesn't block others
- **Unidirectional streams** — server-to-client or client-to-server, one-way
- **Unreliable datagrams** — low-latency messages that may be dropped or reordered, like UDP

This makes WebTransport well-suited for applications where WebSockets fall short: games, real-time audio/video signaling, live telemetry, and anything where head-of-line blocking or connection setup latency matters.

WebTransport requires an HTTP/3-capable server. See [Runtime Adapters, TLS, and HTTP/3](../overview/runtimes.md) for setup.

## Handler

Add `WebTransport` to your handler chain. It intercepts incoming WebTransport CONNECT requests and hands each session to your function:

```rust
use trillium_webtransport::{WebTransport, WebTransportConnection};

let app = WebTransport::new(|wt: WebTransportConnection| async move {
    while let Some(stream) = wt.accept_next_stream().await {
        // handle inbound bidi or uni stream
        drop(stream);
    }
});
```

Other handlers in the chain that precede `WebTransport` run normally for non-WebTransport requests, so you can serve HTTP and WebTransport from the same handler tuple.

## WebTransportConnection API

Each session gets a `WebTransportConnection` with these methods:

| Method | Description |
|--------|-------------|
| `accept_bidi()` | Wait for the next client-initiated bidirectional stream |
| `accept_uni()` | Wait for the next client-initiated unidirectional stream |
| `accept_next_stream()` | Wait for either kind of inbound stream |
| `open_bidi()` | Open a server-initiated bidirectional stream |
| `open_uni()` | Open a server-initiated unidirectional stream |
| `recv_datagram()` | Receive an unreliable datagram |
| `send_datagram(&[u8])` | Send an unreliable datagram |

Streams implement `AsyncRead` and `AsyncWrite` (bidi) or just `AsyncRead` (uni inbound) or `AsyncWrite` (uni outbound).

> ℹ️ Run datagram reception in a separate concurrent task from stream acceptance. Datagrams are typically on a latency-sensitive path and shouldn't wait behind slower stream I/O.

## Full server example

```rust
use futures_lite::{AsyncReadExt, AsyncWriteExt};
use trillium_quinn::QuicConfig;
use trillium_rustls::RustlsAcceptor;
use trillium_webtransport::{InboundStream, WebTransport, WebTransportConnection};

async fn handle(wt: WebTransportConnection) {
    // Echo datagrams back to the client
    let datagram_loop = async {
        while let Some(data) = wt.recv_datagram().await {
            let _ = wt.send_datagram(&data);
        }
    };

    // Handle inbound streams
    let stream_loop = async {
        while let Some(stream) = wt.accept_next_stream().await {
            match stream {
                InboundStream::Bidi(mut s) => {
                    let mut buf = Vec::new();
                    if s.read_to_end(&mut buf).await.is_ok_and(|n| n > 0) {
                        let _ = s.write_all(&buf).await; // echo
                    }
                }
                InboundStream::Uni(mut s) => {
                    let mut buf = Vec::new();
                    let _ = s.read_to_end(&mut buf).await;
                }
            }
        }
    };

    futures_lite::future::zip(datagram_loop, stream_loop).await;
}

fn main() {
    let cert = std::fs::read("cert.pem").unwrap();
    let key = std::fs::read("key.pem").unwrap();

    trillium_smol::config()
        .with_acceptor(RustlsAcceptor::from_single_cert(&cert, &key))
        .with_quic(QuicConfig::from_single_cert(&cert, &key))
        .run(WebTransport::new(handle));
}
```

## Datagram buffering

By default, up to 16 datagrams are buffered per session. When the buffer is full, the oldest datagram is dropped to make room for the newest. Adjust this with `with_max_datagram_buffer`:

```rust
// "latest-only" semantics: only the most recent datagram is kept
WebTransport::new(handler).with_max_datagram_buffer(1)
```
