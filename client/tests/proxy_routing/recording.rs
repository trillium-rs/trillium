use super::*;

#[derive(Debug, Default)]
pub(super) struct Records {
    pub(super) hops: Vec<String>,
    pub(super) secure_hops: Vec<(String, bool)>,
    pub(super) requests: Vec<String>,
    pub(super) connect_requests: Vec<String>,
    pub(super) tunnels: Vec<String>,
    pub(super) resolutions: Vec<String>,
    pub(super) fail_connections: bool,
    pub(super) connect_refusal: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub(super) struct RecordingConnector(pub(super) Arc<Mutex<Records>>);

impl Connector for RecordingConnector {
    type Runtime = Runtime;
    type Transport = RecordingTransport;
    type Udp = <trillium_smol::ClientConfig as Connector>::Udp;

    async fn connect(&self, _: &Url) -> io::Result<Self::Transport> {
        panic!("route must use structured destination");
    }

    async fn connect_to(&self, destination: Destination) -> io::Result<Self::Transport> {
        let host = destination.host().unwrap().to_owned();
        if host == "proxy.example" {
            assert_eq!(destination.alpn().unwrap(), [b"http/1.1".as_slice()]);
        }
        let mut records = self.0.lock().unwrap();
        records.hops.push(host.clone());
        records.secure_hops.push((host, destination.secure()));
        if records.fail_connections {
            return Err(io::Error::other("connection rejected"));
        }
        drop(records);
        Ok(RecordingTransport {
            records: self.0.clone(),
            response: VecDeque::new(),
            tunneled: false,
        })
    }

    async fn connect_over(
        &self,
        transport: Box<dyn Transport>,
        destination: Destination,
    ) -> io::Result<Box<dyn Transport>> {
        assert_eq!(transport.negotiated_alpn(), None);
        let host = destination.host().unwrap().to_owned();
        assert_ne!(host, "proxy.example");
        self.0.lock().unwrap().tunnels.push(host);
        Ok(Box::new(RecordingTransport {
            records: self.0.clone(),
            response: VecDeque::new(),
            tunneled: true,
        }))
    }

    fn runtime(&self) -> Runtime {
        trillium_testing::runtime().into()
    }

    async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<std::net::SocketAddr>> {
        self.0.lock().unwrap().resolutions.push(host.to_owned());
        Ok(vec![std::net::SocketAddr::from(([127, 0, 0, 1], port))])
    }
}

#[derive(Debug)]
pub(super) struct RecordingTransport {
    records: Arc<Mutex<Records>>,
    response: VecDeque<u8>,
    tunneled: bool,
}

impl AsyncRead for RecordingTransport {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if self.response.is_empty() {
            return Poll::Pending;
        }
        let len = buf.len().min(self.response.len());
        for byte in &mut buf[..len] {
            *byte = self.response.pop_front().unwrap();
        }
        Poll::Ready(Ok(len))
    }
}

impl AsyncWrite for RecordingTransport {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let text = String::from_utf8_lossy(buf).into_owned();
        if !self.tunneled && text.starts_with("CONNECT origin.example:") {
            let mut records = self.records.lock().unwrap();
            records.connect_requests.push(text);
            let refusal = records.connect_refusal.clone();
            drop(records);
            self.response
                .extend(refusal.unwrap_or_else(|| b"HTTP/1.1 200 OK\r\n\r\n".to_vec()));
        } else {
            self.records.lock().unwrap().requests.push(text);
            self.response
                .extend(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl Transport for RecordingTransport {
    fn negotiated_alpn(&self) -> Option<Cow<'_, [u8]>> {
        (!self.tunneled).then_some(Cow::Borrowed(b"http/1.1"))
    }
}

pub(super) fn proxy(secret: &str) -> Proxy {
    Proxy::new("http://proxy.example:8080")
        .unwrap()
        .with_connect_header(KnownHeaderName::ProxyAuthorization, secret.to_owned())
        .with_connect_header("x-connect-only", "private")
}

pub(super) fn fixture() -> (Client, Arc<Mutex<Records>>) {
    let records = Arc::new(Mutex::new(Records::default()));
    (Client::new(RecordingConnector(records.clone())), records)
}
