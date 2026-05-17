use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::CertificateDer;
use std::{future::Future, io, sync::Arc};
use trillium_http::{
    Conn, HttpContext, Upgrade,
    h3::{H3Connection, H3Error, H3ErrorCode, H3StreamResult, UniStreamResult},
};
use trillium_quinn::{QuicConfig, QuinnTransport};
use trillium_server_common::{
    Info, QuicConfig as QuicConfigTrait, QuicConnectionTrait, QuicEndpoint, QuicTransportReceive,
    QuicTransportSend, Server,
};
use trillium_smol::{SmolRuntime, SmolTransport, SmolUdpSocket, async_net::TcpStream};
use trillium_testing::{Runtime, runtime};

/// Phantom [`Server`] used solely as the type witness for
/// `<QuicConfig as QuicConfigTrait<S>>::bind`. We never construct one — the
/// trait impl only needs `S::Runtime` + `S::UdpTransport` to satisfy
/// trillium-quinn's bounds; none of the `Server` methods are reachable from
/// the bind call.
enum FixtureServer {}

impl Server for FixtureServer {
    type Runtime = SmolRuntime;
    type Transport = SmolTransport<TcpStream>;
    type UdpTransport = SmolUdpSocket;

    fn runtime() -> Self::Runtime {
        unreachable!("FixtureServer is uninhabited")
    }

    async fn accept(&mut self) -> io::Result<Self::Transport> {
        match *self {}
    }

    fn from_host_and_port(_host: &str, _port: u16) -> Self {
        unreachable!("FixtureServer is uninhabited")
    }
}

/// E2e test fixture that runs trillium-http's HTTP/3 driver against a real quinn endpoint
/// bound to `127.0.0.1:0`. Bypasses `trillium::Handler` and `trillium-server-common`'s
/// h3 driver orchestration; tests provide a `Fn(Conn<QuinnTransport>) -> Fut` directly,
/// plus an optional upgrade closure for tests that exercise the `Conn::upgrade()` →
/// `Upgrade` path.
///
/// Each fixture generates a fresh self-signed certificate for `localhost`; the test
/// retrieves it via [`H3Server::cert_der`] and trusts it in the trillium-client
/// configuration.
#[derive(Debug)]
pub struct H3Server {
    base_url: String,
    cert_der: CertificateDer<'static>,
    context: Arc<HttpContext>,
}

impl H3Server {
    pub async fn new<H, HFut>(handler: H) -> Self
    where
        H: Fn(Conn<QuinnTransport>) -> HFut + Send + Sync + 'static,
        HFut: Future<Output = Conn<QuinnTransport>> + Send + 'static,
    {
        Self::with_upgrade(handler, noop_upgrade).await
    }

    pub async fn with_upgrade<H, HFut, U, UFut>(handler: H, upgrade_handler: U) -> Self
    where
        H: Fn(Conn<QuinnTransport>) -> HFut + Send + Sync + 'static,
        HFut: Future<Output = Conn<QuinnTransport>> + Send + 'static,
        U: Fn(Upgrade<QuinnTransport>) -> UFut + Send + Sync + 'static,
        UFut: Future<Output = ()> + Send + 'static,
    {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_pem = cert.pem().into_bytes();
        let key_pem = signing_key.serialize_pem().into_bytes();
        let cert_der = cert.der().clone();

        let quic = QuicConfig::from_single_cert(&cert_pem, &key_pem);
        let rt = SmolRuntime::default();
        let mut info = Info::default();
        // Resolve "localhost" so the OS picks whichever address it prefers (127.0.0.1 on macOS
        // via /etc/hosts), so the client — which connects via "localhost:{port}" for SNI — hits
        // the same address. Binding to "127.0.0.1:0" can mismatch with a localhost client that
        // resolves IPv6 first.
        let resolved: std::net::SocketAddr = {
            use std::net::ToSocketAddrs;
            ("localhost", 0)
                .to_socket_addrs()
                .expect("resolve localhost")
                .next()
                .expect("at least one address for localhost")
        };
        let endpoint =
            <QuicConfig as QuicConfigTrait<FixtureServer>>::bind(quic, resolved, rt, &mut info)
                .expect("QuicConfig::bind returned None")
                .expect("QuicConfig::bind failed");
        let addr = endpoint.local_addr().expect("local_addr");
        let base_url = format!("https://localhost:{}/", addr.port());

        let context = Arc::new(HttpContext::new());
        let handler = Arc::new(handler);
        let upgrade_handler = Arc::new(upgrade_handler);
        let trillium_rt: Runtime = runtime().into();

        let endpoint = Arc::new(endpoint);
        let swansong = context.swansong().clone();
        let context_for_loop = context.clone();
        let trillium_rt_for_loop = trillium_rt.clone();

        trillium_rt.spawn(async move {
            while let Some(connection) = swansong.interrupt(endpoint.accept()).await.flatten() {
                let h3 = H3Connection::new(context_for_loop.clone());
                let connection = Arc::new(connection);
                let handler = handler.clone();
                let upgrade_handler = upgrade_handler.clone();
                let trillium_rt = trillium_rt_for_loop.clone();
                trillium_rt.clone().spawn(run_connection(
                    connection,
                    h3,
                    handler,
                    upgrade_handler,
                    trillium_rt,
                ));
            }
        });

        Self {
            base_url,
            cert_der,
            context,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn cert_der(&self) -> &CertificateDer<'static> {
        &self.cert_der
    }

    pub async fn shut_down(self) {
        self.context.shut_down().await;
    }
}

async fn noop_upgrade<T>(_: Upgrade<T>) {}

async fn run_connection<C, H, HFut, U, UFut>(
    connection: Arc<C>,
    h3: Arc<H3Connection>,
    handler: Arc<H>,
    upgrade_handler: Arc<U>,
    rt: Runtime,
) where
    C: QuicConnectionTrait + 'static,
    H: Fn(Conn<C::BidiStream>) -> HFut + Send + Sync + 'static,
    HFut: Future<Output = Conn<C::BidiStream>> + Send + 'static,
    U: Fn(Upgrade<C::BidiStream>) -> UFut + Send + Sync + 'static,
    UFut: Future<Output = ()> + Send + 'static,
{
    spawn_outbound_control(&connection, &h3, &rt);
    spawn_qpack_encoder(&connection, &h3, &rt);
    spawn_qpack_decoder(&connection, &h3, &rt);
    spawn_inbound_uni(&connection, &h3, &rt);
    accept_bidi_loop(connection, h3, handler, upgrade_handler, rt).await;
}

async fn accept_bidi_loop<C, H, HFut, U, UFut>(
    connection: Arc<C>,
    h3: Arc<H3Connection>,
    handler: Arc<H>,
    upgrade_handler: Arc<U>,
    rt: Runtime,
) where
    C: QuicConnectionTrait + 'static,
    H: Fn(Conn<C::BidiStream>) -> HFut + Send + Sync + 'static,
    HFut: Future<Output = Conn<C::BidiStream>> + Send + 'static,
    U: Fn(Upgrade<C::BidiStream>) -> UFut + Send + Sync + 'static,
    UFut: Future<Output = ()> + Send + 'static,
{
    loop {
        match h3.swansong().interrupt(connection.accept_bidi()).await {
            None => break,
            Some(Err(_)) => break,
            Some(Ok((stream_id, transport))) => {
                let connection = connection.clone();
                let h3 = h3.clone();
                let handler = handler.clone();
                let upgrade_handler = upgrade_handler.clone();
                rt.spawn(async move {
                    let handler_fn = {
                        let handler = handler.clone();
                        move |conn: Conn<_>| {
                            let handler = handler.clone();
                            async move { handler(conn).await }
                        }
                    };
                    let result = h3
                        .clone()
                        .process_inbound_bidi_with_reset(
                            transport,
                            handler_fn,
                            stream_id,
                            |t, code| {
                                let raw = u64::from(code);
                                t.stop(raw);
                                t.reset(raw);
                            },
                        )
                        .await;

                    match result {
                        Ok(H3StreamResult::Request(conn)) if conn.should_upgrade() => {
                            upgrade_handler(Upgrade::from(conn)).await;
                        }
                        Ok(H3StreamResult::Request(_)) => {}
                        Ok(H3StreamResult::WebTransport { mut transport, .. }) => {
                            transport.stop(H3ErrorCode::StreamCreationError.into());
                            transport.reset(H3ErrorCode::StreamCreationError.into());
                        }
                        Err(error) => handle_error(error, &*connection, &h3),
                    }
                });
            }
        }
    }
    h3.shut_down();
}

fn spawn_inbound_uni<C>(connection: &Arc<C>, h3: &Arc<H3Connection>, rt: &Runtime)
where
    C: QuicConnectionTrait + 'static,
{
    let (connection, h3, rt) = (connection.clone(), h3.clone(), rt.clone());
    rt.clone().spawn(async move {
        while let Some(Ok((_id, recv))) = h3.swansong().interrupt(connection.accept_uni()).await {
            let (connection, h3) = (connection.clone(), h3.clone());
            rt.spawn(async move {
                let close_connection = {
                    let connection = connection.clone();
                    let h3 = h3.clone();
                    move |code: H3ErrorCode| {
                        connection.close(code.into(), code.reason().as_bytes());
                        h3.shut_down();
                    }
                };
                let result = h3
                    .process_inbound_uni_with_close(recv, close_connection)
                    .await;
                match result {
                    Ok(UniStreamResult::Handled) => {}
                    Ok(UniStreamResult::WebTransport { mut stream, .. }) => {
                        stream.stop(H3ErrorCode::StreamCreationError.into());
                    }
                    Ok(UniStreamResult::Unknown { mut stream, .. }) => {
                        stream.stop(H3ErrorCode::StreamCreationError.into());
                    }
                    Err(error) => handle_error(error, &*connection, &h3),
                }
            });
        }
        h3.shut_down();
    });
}

fn spawn_outbound_control<C>(connection: &Arc<C>, h3: &Arc<H3Connection>, rt: &Runtime)
where
    C: QuicConnectionTrait + 'static,
{
    let (connection, h3) = (connection.clone(), h3.clone());
    rt.spawn(async move {
        if let Ok((_id, stream)) = connection.open_uni().await
            && let Err(error) = h3.run_outbound_control(stream).await
        {
            handle_error(error, &*connection, &h3);
        }
        h3.shut_down();
    });
}

fn spawn_qpack_encoder<C>(connection: &Arc<C>, h3: &Arc<H3Connection>, rt: &Runtime)
where
    C: QuicConnectionTrait + 'static,
{
    let (connection, h3) = (connection.clone(), h3.clone());
    rt.spawn(async move {
        if let Ok((_id, stream)) = connection.open_uni().await
            && let Err(error) = h3.run_encoder(stream).await
        {
            handle_error(error, &*connection, &h3);
        }
        h3.shut_down();
    });
}

fn spawn_qpack_decoder<C>(connection: &Arc<C>, h3: &Arc<H3Connection>, rt: &Runtime)
where
    C: QuicConnectionTrait + 'static,
{
    let (connection, h3) = (connection.clone(), h3.clone());
    rt.spawn(async move {
        if let Ok((_id, stream)) = connection.open_uni().await
            && let Err(error) = h3.run_decoder(stream).await
        {
            handle_error(error, &*connection, &h3);
        }
        h3.shut_down();
    });
}

fn handle_error<C: QuicConnectionTrait>(error: H3Error, connection: &C, h3: &H3Connection) {
    if let H3Error::Protocol(code) = error
        && code.is_connection_error()
    {
        connection.close(code.into(), code.reason().as_bytes());
        h3.shut_down();
    }
}
