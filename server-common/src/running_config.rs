use crate::{Acceptor, ArcHandler, RuntimeTrait, Server, h2};
use futures_lite::{AsyncReadExt, AsyncWriteExt};
use std::{io::ErrorKind, net::SocketAddr, sync::Arc};
use trillium::{Conn, Handler, KnownHeaderName, Transport};
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
    /// The bound address of the listener feeding this loop, stamped into each conn's state as the
    /// ingress (local) address. `None` if unknown.
    pub(crate) local_addr: Option<SocketAddr>,
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

        self.context.swansong().shut_down().await;
        listener.clean_up().await;
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

        let peer_ip = stream.peer_addr().ok().flatten().map(|addr| addr.ip());
        let local_addr = self.local_addr;
        let local_alt_svc = self.local_alt_svc;

        let mut transport = match self.acceptor.accept(stream).await {
            Ok(stream) => stream,
            Err(e) => {
                log::error!("acceptor error: {:?}", e);
                return;
            }
        };

        let is_secure = self.acceptor.is_secure();
        let runtime: crate::Runtime = self.runtime.clone().into();

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
                h2::run_h2(
                    transport,
                    self.context.clone(),
                    handler,
                    runtime,
                    peer_ip,
                    local_addr,
                    local_alt_svc,
                    is_secure,
                )
                .await;
                return;
            }
            AlpnDispatch::H1 => {
                let handler_ref = &handler;
                let result = self
                    .context
                    .clone()
                    .run(transport, |mut conn| async move {
                        conn.set_peer_ip(peer_ip);
                        let mut conn = Conn::from(conn);
                        if let Some(local_addr) = local_addr {
                            conn.insert_state(local_addr);
                        }
                        if let Some(alt_svc) = local_alt_svc {
                            conn.response_headers_mut()
                                .try_insert(KnownHeaderName::AltSvc, alt_svc);
                        }
                        let conn = handler_ref.run(conn).await;
                        let conn = handler_ref.before_send(conn).await;
                        conn.into_inner()
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
            h2::run_h2(
                transport,
                self.context.clone(),
                handler,
                runtime,
                peer_ip,
                local_addr,
                local_alt_svc,
                is_secure,
            )
            .await;
            return;
        }
        let handler_ref = &handler;
        let result = trillium_http::run_with_initial_bytes(
            self.context.clone(),
            transport,
            peek,
            |mut conn| async move {
                conn.set_peer_ip(peer_ip);
                let mut conn = Conn::from(conn);
                if let Some(local_addr) = local_addr {
                    conn.insert_state(local_addr);
                }
                if let Some(alt_svc) = local_alt_svc {
                    conn.response_headers_mut()
                        .try_insert(KnownHeaderName::AltSvc, alt_svc);
                }
                let conn = handler_ref.run(conn).await;
                let conn = handler_ref.before_send(conn).await;
                conn.into_inner()
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

        Err(Error::Io(e))
            if e.kind() == ErrorKind::ConnectionReset || e.kind() == ErrorKind::BrokenPipe =>
        {
            log::debug!("closing connection");
        }

        Err(Error::Io(ref e))
            if e.kind() == ErrorKind::UnexpectedEof
                && e.get_ref()
                    .is_some_and(|inner| inner.to_string().contains("TLS close_notify")) =>
        {
            log::debug!("closing connection (tls client did not close notify)");
        }

        Err(e) => {
            log::error!("http error: {:?}", e);
        }
    };
}
