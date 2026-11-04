use super::*;

#[derive(Clone, Debug)]
pub(super) struct H2Connector {
    pub(super) routes: Arc<Mutex<Vec<String>>>,
}

impl H2Connector {
    fn spawn_h2_server(runtime: &Runtime, server: trillium_testing::TestTransport) {
        let stream_runtime = runtime.clone();
        runtime.spawn_detached(async move {
            let h2 = trillium_http::h2::H2Connection::new(Arc::default());
            let mut driver = h2.run(server);
            while let Some(result) = driver.next().await {
                let conn = result.unwrap();
                stream_runtime.spawn_detached(async move {
                    trillium_http::h2::H2Connection::process_inbound(conn, |conn| async move {
                        conn.with_status(200).with_response_body("h2 body")
                    })
                    .await
                    .unwrap();
                });
            }
        });
    }
}

impl Connector for H2Connector {
    type Runtime = Runtime;
    type Transport = H2Transport;
    type Udp = ();

    async fn connect(&self, _: &Url) -> io::Result<Self::Transport> {
        panic!("route must use structured destination");
    }

    async fn connect_to(&self, destination: Destination) -> io::Result<Self::Transport> {
        let (client, mut server) = trillium_testing::TestTransport::new();
        let runtime = self.runtime();
        match destination.host() {
            Some("origin.example") => {
                assert!(destination.secure());
                self.routes.lock().unwrap().push("direct".into());
                Self::spawn_h2_server(&runtime, server);
                Ok(H2Transport {
                    inner: Box::new(client),
                    alpn: Some(b"h2"),
                })
            }
            Some("proxy.example") => {
                assert!(!destination.secure());
                assert_eq!(destination.alpn().unwrap(), [b"http/1.1".as_slice()]);
                let routes = self.routes.clone();
                let h2_runtime = runtime.clone();
                runtime.spawn_detached(async move {
                    let mut head = Vec::new();
                    while !head.ends_with(b"\r\n\r\n") {
                        let mut byte = [0];
                        server.read_exact(&mut byte).await.unwrap();
                        head.push(byte[0]);
                    }
                    let line_end = head.windows(2).position(|w| w == b"\r\n").unwrap();
                    assert!(matches!(
                        &head[..line_end],
                        b"CONNECT origin.example:443 HTTP/1.1"
                            | b"CONNECT origin.example:80 HTTP/1.1"
                    ));
                    let headers = Headers::parse(&head[line_end + 2..head.len() - 2]).unwrap();
                    routes.lock().unwrap().push(
                        headers
                            .get_str(KnownHeaderName::ProxyAuthorization)
                            .unwrap()
                            .to_owned(),
                    );
                    server.write_all(b"HTTP/1.1 200 OK\r\n\r\n");
                    Self::spawn_h2_server(&h2_runtime, server);
                });
                Ok(H2Transport {
                    inner: Box::new(client),
                    alpn: Some(b"http/1.1"),
                })
            }
            other => panic!("unexpected destination host: {other:?}"),
        }
    }

    async fn connect_over(
        &self,
        transport: Box<dyn Transport>,
        destination: Destination,
    ) -> io::Result<Box<dyn Transport>> {
        assert_eq!(destination.host(), Some("origin.example"));
        assert!(destination.addrs().is_empty());
        assert_eq!(transport.negotiated_alpn(), None);
        Ok(Box::new(H2Transport {
            inner: transport,
            alpn: destination.secure().then_some(b"h2"),
        }))
    }

    async fn resolve(&self, _: &str, port: u16) -> io::Result<Vec<std::net::SocketAddr>> {
        Ok(vec![std::net::SocketAddr::from(([127, 0, 0, 1], port))])
    }

    fn runtime(&self) -> Runtime {
        trillium_testing::runtime().into()
    }
}

#[derive(trillium_macros::AsyncRead, trillium_macros::AsyncWrite, trillium_macros::Transport)]
pub(super) struct H2Transport {
    #[async_io]
    #[transport(except = negotiated_alpn)]
    inner: Box<dyn Transport>,
    alpn: Option<&'static [u8]>,
}
impl std::fmt::Debug for H2Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("H2Transport")
            .field("alpn", &self.alpn)
            .finish_non_exhaustive()
    }
}
impl H2Transport {
    fn negotiated_alpn(&self) -> Option<Cow<'_, [u8]>> {
        self.alpn.map(Cow::Borrowed)
    }
}
