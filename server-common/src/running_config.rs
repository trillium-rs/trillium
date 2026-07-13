use crate::{Acceptor, ArcHandler, RuntimeTrait, Server, h2, unmap_ipv4};
use futures_lite::{AsyncReadExt, AsyncWriteExt};
use std::{io::ErrorKind, sync::Arc};
use trillium::{Conn, Handler, KnownHeaderName, Listener, Transport};
use trillium_http::{Error, HttpContext, SERVICE_UNAVAILABLE, Upgrade};

/// How a freshly-accepted connection should be dispatched based on ALPN result.
#[derive(Clone, Copy)]
enum AlpnDispatch {
    /// ALPN explicitly negotiated `h2` — run HTTP/2 directly.
    H2,
    /// ALPN explicitly negotiated `http/1.1` — respect that, run HTTP/1 without peeking.
    H1,
    /// ALPN absent or returned something else — peek the first 24 bytes for the HTTP/2
    /// client preface (RFC 9113 §3.4) to decide between prior-knowledge h2 and HTTP/1.
    PrefacePeek,
}

#[derive(Debug)]
pub struct RunningConfig<ServerType: Server, AcceptorType> {
    pub(crate) acceptor: AcceptorType,
    pub(crate) max_connections: Option<usize>,
    pub(crate) nodelay: bool,
    pub(crate) runtime: ServerType::Runtime,
    pub(crate) context: Arc<HttpContext>,
    /// The listener feeding this loop, stamped into each conn's state as its provenance (and, for
    /// an addressed listener, its [`SocketAddr`](std::net::SocketAddr) as the ingress
    /// address). `None` if unknown.
    pub(crate) listener: Option<Listener>,
    /// `alt-svc` header value to set on every response from this listener, if any. Pre-merged
    /// (multiple alternatives joined into one comma-separated value) and `'static` so it can be
    /// shared cheaply and benefits from h2/h3 dynamic-table reuse.
    pub(crate) local_alt_svc: Option<&'static str>,
}

impl<S: Server, A: Acceptor<<S as Server>::Transport>> RunningConfig<S, A> {
    pub(crate) async fn run_async(
        self: Arc<Self>,
        mut listener: S,
        handler: ArcHandler<impl Handler>,
    ) {
        let swansong = self.context.as_ref().swansong();
        let runtime = self.runtime.clone();
        let clean_up_guard = swansong.guard();
        while let Some(transport) = swansong.interrupt(listener.accept()).await {
            match transport {
                Ok(stream) => {
                    runtime.spawn(
                        Arc::clone(&self).handle_stream(stream, ArcHandler::clone(&handler)),
                    );
                }
                Err(e) => log::error!("tcp error: {}", e),
            }
        }

        listener.clean_up().await;
        drop(clean_up_guard);
        swansong.shut_down().await;
    }

    async fn handle_stream(
        self: Arc<Self>,
        mut stream: S::Transport,
        handler: ArcHandler<impl Handler>,
    ) {
        if self.over_capacity() {
            let mut byte = [0u8]; // wait for the client to start requesting
            trillium::log_error!(stream.read(&mut byte).await);
            trillium::log_error!(stream.write_all(SERVICE_UNAVAILABLE).await);
            return;
        }

        trillium::log_error!(stream.set_nodelay(self.nodelay));

        let peer_ip = stream
            .peer_addr()
            .ok()
            .flatten()
            .map(|addr| unmap_ipv4(addr.ip()));
        let listener = self.listener.clone();
        let local_alt_svc = self.local_alt_svc;
        // Stamped onto each conn like h2/h3 do (see `run_h2` / `run_h3`): trillium-http is
        // TLS-agnostic and builds conns with `secure = false`, so the acceptor is the only thing
        // that knows whether this connection is secure.
        let is_secure = self.acceptor.is_secure();

        let mut transport = match self.acceptor.accept(stream).await {
            Ok(stream) => stream,
            Err(e) => {
                // The peer is named because a connection that fails to accept
                // dies before any `Conn` exists, so no handler can ever observe
                // it. This line is the only place a repeated source of
                // handshake failures — which no browser produces — can be
                // attributed to a host.
                match peer_ip {
                    Some(ip) => log::error!("acceptor error from {ip}: {e:?}"),
                    None => log::error!("acceptor error from unknown peer: {e:?}"),
                }
                return;
            }
        };

        // Dispatch by ALPN if present:
        //   - `h2`        → run HTTP/2 directly
        //   - `http/1.1`  → respect the negotiation, run HTTP/1 without peeking
        //   - absent / anything else → fall through to prior-knowledge preface peek
        let alpn_dispatch = match transport.negotiated_alpn().as_deref() {
            Some(b"h2") => AlpnDispatch::H2,
            Some(b"http/1.1") => AlpnDispatch::H1,
            _ => AlpnDispatch::PrefacePeek,
        };

        match alpn_dispatch {
            AlpnDispatch::H2 => {
                self.run_h2(transport, handler, peer_ip).await;
                return;
            }
            AlpnDispatch::H1 => {
                let handler_ref = &handler;
                let result = self
                    .context
                    .clone()
                    .run(transport, |mut conn| {
                        // cloned per request: this closure is `FnMut`, invoked once per keep-alive
                        // request, and each `async move` body takes ownership of its provenance.
                        let listener = listener.clone();
                        async move {
                            conn.set_peer_ip(peer_ip);
                            conn.set_secure(is_secure);
                            let mut conn = Conn::from(conn);
                            stamp_listener(&mut conn, listener);
                            if let Some(alt_svc) = local_alt_svc {
                                conn.response_headers_mut()
                                    .try_insert(KnownHeaderName::AltSvc, alt_svc);
                            }
                            let conn = handler_ref.run(conn).await;
                            let conn = handler_ref.before_send(conn).await;
                            conn.into_inner()
                        }
                    })
                    .await;
                handle_h1_result(result, &handler).await;
                return;
            }
            AlpnDispatch::PrefacePeek => {}
        }

        // Prior-knowledge HTTP/2: peek the first 24 bytes for the preface. Applies to both
        // cleartext (h2c) and TLS-without-ALPN-h2 (e.g. clients on TLS stacks that don't
        // expose an ALPN knob). Bail to HTTP/1 on any mismatch or premature EOF, handing the
        // peeked bytes into the parser.
        let peek = match peek_preface(&mut transport).await {
            Ok(bytes) => bytes,
            Err(e) => {
                log::debug!("preface peek error: {e}");
                return;
            }
        };
        if peek.as_slice() == h2::CLIENT_PREFACE {
            let transport = h2::Prefixed::new(peek, transport);
            self.run_h2(transport, handler, peer_ip).await;
            return;
        }
        let handler_ref = &handler;
        let result = trillium_http::run_with_initial_bytes(
            self.context.clone(),
            transport,
            peek,
            |mut conn| {
                let listener = listener.clone();
                async move {
                    conn.set_peer_ip(peer_ip);
                    conn.set_secure(is_secure);
                    let mut conn = Conn::from(conn);
                    stamp_listener(&mut conn, listener);
                    if let Some(alt_svc) = local_alt_svc {
                        conn.response_headers_mut()
                            .try_insert(KnownHeaderName::AltSvc, alt_svc);
                    }
                    let conn = handler_ref.run(conn).await;
                    let conn = handler_ref.before_send(conn).await;
                    conn.into_inner()
                }
            },
        )
        .await;
        handle_h1_result(result, &handler).await;
    }

    fn over_capacity(&self) -> bool {
        self.max_connections
            .is_some_and(|m| self.context.swansong().guard_count() >= m)
    }
}

/// Stamp a conn's listener provenance: the [`Listener`] itself, plus its
/// [`SocketAddr`](std::net::SocketAddr) as the ingress address when the listener is addressed
/// (TCP/QUIC, not a Unix socket).
fn stamp_listener(conn: &mut Conn, listener: Option<Listener>) {
    if let Some(listener) = listener {
        if let Some(addr) = listener.socket_addr() {
            conn.insert_state(addr);
        }
        conn.insert_state(listener);
    }
}

/// Incrementally read up to 24 bytes from the transport, bailing as soon as the bytes
/// collected no longer form a prefix of the HTTP/2 client preface. On success, returns either
/// (a) exactly 24 bytes that match the preface, or (b) fewer than 24 bytes that should be
/// handed to the HTTP/1 parser via
/// [`run_with_initial_bytes`][trillium_http::run_with_initial_bytes].
///
/// The incremental shape matters because an HTTP/1 request can legitimately consist of fewer
/// than 24 bytes (e.g. `GET /\r\nHost:x\r\n\r\n` is 20 bytes). Waiting unconditionally for 24
/// bytes could deadlock a valid HTTP/1 client that's already sent its full request.
async fn peek_preface<T>(transport: &mut T) -> std::io::Result<Vec<u8>>
where
    T: AsyncReadExt + Unpin,
{
    let mut peek = vec![0u8; 24];
    let mut filled = 0;
    while filled < 24 {
        let n = transport.read(&mut peek[filled..]).await?;
        if n == 0 {
            break;
        }
        filled += n;
        if !h2::CLIENT_PREFACE.starts_with(&peek[..filled]) {
            break;
        }
    }
    peek.truncate(filled);
    Ok(peek)
}

async fn handle_h1_result<T, H: Handler>(
    result: Result<Option<Upgrade<T>>, Error>,
    handler: &ArcHandler<H>,
) where
    T: Transport + 'static,
{
    match result {
        Ok(Some(upgrade)) => {
            let upgrade = upgrade.into();
            if handler.has_upgrade(&upgrade) {
                log::debug!("upgrading...");
                handler.upgrade(upgrade).await;
            } else {
                log::error!("upgrade specified but no upgrade handler provided");
            }
        }

        Err(Error::Closed) | Ok(None) => {
            log::debug!("closing connection");
        }

        // UnexpectedEof at the connection loop always means the peer vanished mid-connection —
        // a TLS client that skipped close_notify, or a plaintext client that dropped mid-request.
        // Peer misbehavior, not a server error.
        Err(Error::Io(e))
            if matches!(
                e.kind(),
                ErrorKind::ConnectionReset | ErrorKind::BrokenPipe | ErrorKind::UnexpectedEof
            ) =>
        {
            log::debug!("closing connection ({e})");
        }

        Err(e) => {
            log::error!("http error: {:?}", e);
        }
    };
}
