use super::*;
use crate::Url;
use futures_lite::AsyncReadExt;
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use trillium_server_common::Runtime;
use trillium_testing::{TestTransport, harness, test};

#[derive(Clone)]
struct InMemoryProxy {
    response: Vec<u8>,
    dial_count: Arc<AtomicUsize>,
    server: Arc<std::sync::Mutex<Option<TestTransport>>>,
}

impl InMemoryProxy {
    fn new(response: Vec<u8>) -> Self {
        Self {
            response,
            dial_count: Arc::new(AtomicUsize::new(0)),
            server: Arc::new(std::sync::Mutex::new(None)),
        }
    }
}

#[derive(
    Debug, trillium_macros::AsyncRead, trillium_macros::AsyncWrite, trillium_macros::Transport,
)]
struct ProxyTransport(#[transport(except = negotiated_alpn)] TestTransport);

impl ProxyTransport {
    const fn negotiated_alpn(&self) -> Option<Cow<'_, [u8]>> {
        Some(Cow::Borrowed(b"http/1.1"))
    }
}

impl Connector for InMemoryProxy {
    type Runtime = Runtime;
    type Transport = ProxyTransport;
    type Udp = ();

    async fn connect(&self, _: &Url) -> io::Result<Self::Transport> {
        unreachable!()
    }

    async fn connect_to(&self, destination: Destination) -> io::Result<Self::Transport> {
        assert_eq!(destination.alpn().unwrap(), [b"http/1.1".as_slice()]);
        self.dial_count.fetch_add(1, Ordering::SeqCst);
        let (client, server) = TestTransport::new();
        server.write_all(&self.response);
        server.shutdown(std::net::Shutdown::Write);
        *self.server.lock().unwrap() = Some(server);
        Ok(ProxyTransport(client))
    }

    fn runtime(&self) -> Runtime {
        trillium_testing::runtime().into()
    }

    async fn resolve(&self, _: &str, _: u16) -> io::Result<Vec<SocketAddr>> {
        unreachable!()
    }
}

fn head_max_len() -> usize {
    HttpConfig::default().head_max_len()
}

fn into_io_error(error: Error) -> io::Error {
    match error {
        Error::Io(io_error) => io_error,
        other => panic!("expected Error::Io, got {other:?}"),
    }
}

async fn run_proxy(
    response: Vec<u8>,
    origin: Destination,
) -> Result<(Vec<u8>, Vec<u8>, Option<Cow<'static, [u8]>>)> {
    let connector = InMemoryProxy::new(response);
    let proxy = Proxy::new("http://proxy.example:8080")
        .unwrap()
        .with_connect_header(KnownHeaderName::ProxyAuthorization, "Basic secret");
    let mut stream = open_tunnel(&connector, &proxy, &origin, &HttpConfig::default()).await?;
    let alpn = stream.negotiated_alpn().map(|a| Cow::Owned(a.into_owned()));
    let mut payload = Vec::new();
    stream.read_to_end(&mut payload).await?;
    drop(stream);
    let mut server = connector.server.lock().unwrap().take().unwrap();
    let mut request = Vec::new();
    server.read_to_end(&mut request).await?;
    Ok((request, payload, alpn))
}

#[test(harness)]
async fn informational_and_success_leave_coalesced_origin_bytes_untouched() {
    let response = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 299 Tunnel\r\nContent-Length: 99\r\n\r\norigin bytes".to_vec();
    let (request, payload, alpn) = run_proxy(
        response,
        Destination::new_with_host(true, "origin.example", 443),
    )
    .await
    .unwrap();
    assert_eq!(
        request,
        b"CONNECT origin.example:443 HTTP/1.1\r\nHost: origin.example:443\r\nProxy-Authorization: Basic secret\r\n\r\n"
    );
    assert_eq!(payload, b"origin bytes");
    assert_eq!(alpn, None);
}

#[test(harness)]
async fn origin_bytes_continue_from_the_transport_after_the_leftover() {
    let origin_bytes = b"0123456789".repeat(1000);
    let response = [b"HTTP/1.1 200 OK\r\n\r\n".as_slice(), &origin_bytes].concat();
    let (_, payload, _) = run_proxy(
        response,
        Destination::new_with_host(true, "origin.example", 443),
    )
    .await
    .unwrap();
    assert_eq!(payload, origin_bytes);
}

#[test(harness)]
async fn ipv6_authority_is_bracketed() {
    let (request, _, _) = run_proxy(
        b"HTTP/1.0 200 OK\r\n\r\n".to_vec(),
        Destination::new_with_socket_addrs(false, ["[2001:db8::1]:8443".parse().unwrap()]),
    )
    .await
    .unwrap();
    assert!(
        request.starts_with(b"CONNECT [2001:db8::1]:8443 HTTP/1.1\r\nHost: [2001:db8::1]:8443\r\n")
    );
}

#[test(harness)]
async fn interim_heads_share_one_budget() {
    let interim = b"HTTP/1.1 100 Continue\r\n\r\n";
    for response in [
        interim.repeat(head_max_len() / interim.len() + 1),
        [
            b"HTTP/1.1 100 Continue\r\nLong: ".as_slice(),
            &vec![b'x'; head_max_len() / 2],
            b"\r\n\r\nHTTP/1.1 200 OK\r\nLong: ",
            &vec![b'y'; head_max_len() / 2],
            b"\r\n\r\n",
        ]
        .concat(),
    ] {
        let error = run_proxy(
            response,
            Destination::new_with_host(true, "origin.example", 443),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, Error::HeadersTooLong),
            "unexpected error {error:?}"
        );
    }
}

#[test(harness)]
async fn refusals_carry_the_proxy_status() {
    for (code, status, message) in [
        (
            101,
            Status::SwitchingProtocols,
            "CONNECT proxy returned 101 Switching Protocols",
        ),
        (
            407,
            Status::ProxyAuthenticationRequired,
            "CONNECT proxy returned 407 Proxy Authentication Required",
        ),
        (
            499,
            Status::BadRequest,
            "CONNECT proxy returned 400 Bad Request (status code 499)",
        ),
        (
            520,
            Status::InternalServerError,
            "CONNECT proxy returned 500 Internal Server Error (status code 520)",
        ),
        (
            999,
            Status::InternalServerError,
            "CONNECT proxy returned 500 Internal Server Error (status code 999)",
        ),
    ] {
        let err = run_proxy(
            format!("HTTP/1.1 {code} Refused\r\n\r\n").into_bytes(),
            Destination::new_with_host(true, "origin.example", 443),
        )
        .await
        .unwrap_err();
        let err = into_io_error(err);
        assert_eq!(err.kind(), ErrorKind::ConnectionRefused);
        let connect_err = err
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<ProxyConnectError>())
            .expect("a refusal should carry ProxyConnectError");
        assert_eq!(connect_err.status(), status);
        assert_eq!(connect_err.to_string(), message);
    }
}

#[test(harness)]
async fn invalid_connect_inputs_fail_before_dial() {
    let connector = InMemoryProxy::new(b"HTTP/1.1 200 OK\r\n\r\n".to_vec());
    let invalid_input: fn(&Error) -> bool =
        |e| matches!(e, Error::Io(io) if io.kind() == ErrorKind::InvalidInput);
    let reserved: fn(&Error) -> bool = |e| matches!(e, Error::UnexpectedHeader(_));
    let origin = || Destination::new_with_host(true, "origin.example", 443);
    for (origin, headers, expected) in [
        (
            Destination::new_with_host(true, "origin.example/path", 443),
            Headers::new(),
            invalid_input,
        ),
        (
            Destination::new_with_socket_addrs(true, []),
            Headers::new(),
            invalid_input,
        ),
        (
            origin(),
            Headers::from_iter([("content-length", "3")]),
            reserved,
        ),
        (origin(), Headers::from_iter([("UPGRADE", "h2c")]), reserved),
        (
            origin(),
            Headers::from_iter([(
                KnownHeaderName::ProxyAuthorization,
                "Basic secret\r\ninjected: yes",
            )]),
            |e| matches!(e, Error::InvalidHeaderValue(_)),
        ),
        (
            origin(),
            Headers::from_iter([("invalid name", "value")]),
            |e| matches!(e, Error::InvalidHeaderName),
        ),
    ] {
        let proxy = headers.into_iter().fold(
            Proxy::new("https://proxy.example:8443").unwrap(),
            |proxy, (name, values)| proxy.with_connect_header(name, values),
        );
        let error = open_tunnel(&connector, &proxy, &origin, &HttpConfig::default())
            .await
            .err()
            .unwrap();
        assert!(expected(&error), "unexpected error {error:?}");
    }
    assert_eq!(connector.dial_count.load(Ordering::SeqCst), 0);
}
