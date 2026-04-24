use crate::{Acceptor, ArcHandler, RuntimeTrait, Server, h2};
use futures_lite::{AsyncReadExt, AsyncWriteExt};
use std::{io::ErrorKind, sync::Arc};
use trillium::{Handler, Transport};
use trillium_http::{Error, HttpContext, SERVICE_UNAVAILABLE, Upgrade};

#[derive(Debug)]
pub struct RunningConfig<ServerType: Server, AcceptorType> {
    pub(crate) acceptor: AcceptorType,
    pub(crate) max_connections: Option<usize>,
    pub(crate) nodelay: bool,
    pub(crate) runtime: ServerType::Runtime,
    pub(crate) context: Arc<HttpContext>,
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

        let mut transport = match self.acceptor.accept(stream).await {
            Ok(stream) => stream,
            Err(e) => {
                log::error!("acceptor error: {:?}", e);
                return;
            }
        };

        let is_secure = self.acceptor.is_secure();
        let runtime: crate::Runtime = self.runtime.clone().into();

        // ALPN-negotiated HTTP/2 (TLS with `h2` selected).
        let alpn_is_h2 = self
            .acceptor
            .negotiated_alpn(&transport)
            .is_some_and(|alpn| &*alpn == b"h2");
        if alpn_is_h2 {
            h2::run_h2(
                transport,
                self.context.clone(),
                handler,
                runtime,
                peer_ip,
                is_secure,
            )
            .await;
            return;
        }

        // Cleartext prior-knowledge HTTP/2: peek the first 24 bytes for the preface. Bail to
        // HTTP/1 on any mismatch or premature EOF, handing the peeked bytes into the parser.
        if !is_secure {
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
                    false,
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
                    let conn = handler_ref.run(conn.into()).await;
                    let conn = handler_ref.before_send(conn).await;
                    conn.into_inner()
                },
            )
            .await;
            handle_h1_result(result, &handler).await;
            return;
        }

        // TLS without ALPN=h2: run HTTP/1.
        let handler_ref = &handler;
        let result = self
            .context
            .clone()
            .run(transport, |mut conn| async move {
                conn.set_peer_ip(peer_ip);
                let conn = handler_ref.run(conn.into()).await;
                let conn = handler_ref.before_send(conn).await;
                conn.into_inner()
            })
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
