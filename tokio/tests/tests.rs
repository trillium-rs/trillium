use trillium::Swansong;
use trillium_tokio::config;

#[tokio::test]
async fn spawn_async() {
    config().with_port(0).spawn(()).shut_down().await;
}

#[test]
fn spawn_block() {
    config().with_port(0).spawn(()).shut_down().block();
}

#[test]
fn run() {
    let swansong = Swansong::new();
    swansong.shut_down();
    config().with_port(0).with_swansong(swansong).run(());
}

#[tokio::test]
async fn run_async() {
    let swansong = Swansong::new();
    swansong.shut_down();
    config()
        .with_port(0)
        .with_swansong(swansong)
        .run_async(())
        .await;
}

#[tokio::test]
async fn server_builder_serves_and_shuts_down() {
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use trillium::Conn;
    use trillium_tokio::server;

    let handle = server()
        .without_signals()
        .bind_tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap()
        .spawn(|conn: Conn| async move { conn.ok("hello server builder") });

    let addr = *handle.info().await.tcp_socket_addr().unwrap();

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();

    assert!(response.contains("200 OK"), "{response}");
    assert!(response.contains("hello server builder"), "{response}");

    handle.shut_down().await;
}

#[tokio::test]
async fn server_builder_multi_listener() {
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use trillium::Conn;
    use trillium_tokio::server;

    let handle = server()
        .without_signals()
        .bind_tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap()
        .bind_tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap()
        .spawn(|conn: Conn| async move { conn.ok("multi") });

    let addrs = handle.info().await.tcp_addrs().to_vec();
    assert_eq!(addrs.len(), 2, "{addrs:?}");
    assert_ne!(addrs[0], addrs[1]);

    for addr in addrs {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.contains("200 OK"), "addr {addr}: {response}");
        assert!(response.contains("multi"), "addr {addr}: {response}");
    }

    handle.shut_down().await;
}

#[tokio::test]
async fn server_builder_mixed_acceptors() {
    use std::convert::Infallible;
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use trillium::Conn;
    use trillium_server_common::{Acceptor, Transport};
    use trillium_tokio::server;

    // A second, distinct acceptor type so two different concrete acceptors must coexist in one
    // builder (exercising the boxed-acceptor erasure). It passes the transport through unchanged but
    // reports itself secure, so both listeners still speak plaintext HTTP for the test.
    #[derive(Clone)]
    struct Passthrough;
    impl<T: Transport> Acceptor<T> for Passthrough {
        type Output = T;
        type Error = Infallible;
        async fn accept(&self, input: T) -> Result<T, Infallible> {
            Ok(input)
        }
        fn is_secure(&self) -> bool {
            true
        }
    }

    let handle = server()
        .without_signals()
        .bind_tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap()
        .bind_tls(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), Passthrough)
        .unwrap()
        .spawn(|conn: Conn| async move { conn.ok("mixed") });

    let addrs = handle.info().await.tcp_addrs().to_vec();
    assert_eq!(addrs.len(), 2, "{addrs:?}");

    for addr in addrs {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.contains("200 OK"), "addr {addr}: {response}");
        assert!(response.contains("mixed"), "addr {addr}: {response}");
    }

    handle.shut_down().await;
}

#[tokio::test]
async fn server_builder_ingress_local_addr() {
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use trillium::Conn;
    use trillium_tokio::server;

    // The ingress (listener) address is stamped into each conn's state as a `SocketAddr`.
    async fn handler(conn: Conn) -> Conn {
        let body = match conn.state::<SocketAddr>() {
            Some(addr) => format!("local={addr}"),
            None => "local=none".to_string(),
        };
        conn.ok(body)
    }

    let handle = server()
        .without_signals()
        .bind_tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap()
        .bind_tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap()
        .spawn(handler);

    let addrs = handle.info().await.tcp_addrs().to_vec();
    assert_eq!(addrs.len(), 2, "{addrs:?}");

    for addr in addrs {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(
            response.contains(&format!("local={addr}")),
            "conn on {addr} should report its ingress addr: {response}"
        );
    }

    handle.shut_down().await;
}

#[tokio::test]
async fn server_builder_with_alt_svc() {
    use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use trillium::Conn;
    use trillium_tokio::server;

    // Claim a free port, then drop the std listener so the trillium builder can re-bind it.
    // Brief TOCTOU window, but the alternative is a fixed port that risks CI collision.
    let port = StdTcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();

    let handle = server()
        .without_signals()
        .bind_tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
        .unwrap()
        .with_alt_svc(port, 4433)
        .spawn(|conn: Conn| async move { conn.ok("alt-svc") });

    let addr = *handle.info().await.tcp_socket_addr().unwrap();
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();

    assert!(response.contains("200 OK"), "{response}");
    assert!(
        response.to_ascii_lowercase().contains("alt-svc: h3=\":4433\""),
        "expected alt-svc header pointing at port 4433: {response}"
    );

    handle.shut_down().await;
}

#[tokio::test]
async fn server_builder_bind_quic() {
    use std::io;
    use std::net::{Ipv4Addr, SocketAddr, UdpSocket as StdUdpSocket};
    use trillium::{Conn, Info};
    use trillium_server_common::{QuicConfig, Server};
    use trillium_tokio::server;

    // A no-op QuicConfig that produces the `()` QuicEndpoint, whose `accept()` returns `None`
    // immediately. Exercises the builder's bind_quic path (UDP socket claim, endpoint
    // construction inside the runtime, spawn of the h3 accept loop) without pulling in a real
    // QUIC stack.
    struct MockQuic;
    impl<S: Server> QuicConfig<S> for MockQuic {
        type Endpoint = ();
        fn bind(
            self,
            _: SocketAddr,
            _: S::Runtime,
            _: &mut Info,
        ) -> Option<io::Result<()>> {
            Some(Ok(()))
        }
        fn bind_with_socket(
            self,
            _: StdUdpSocket,
            _: S::Runtime,
            _: &mut Info,
        ) -> io::Result<()> {
            Ok(())
        }
    }

    let handle = server()
        .without_signals()
        .bind_tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap()
        .bind_quic(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), MockQuic)
        .unwrap()
        .spawn(|conn: Conn| async move { conn.ok("ok") });

    // If we got here without panicking, the UDP socket was claimed at bind_quic time, the
    // endpoint was built inside the runtime, and the h3 accept loop spawned. Shut down before
    // anything tries to actually exchange QUIC traffic.
    let _info = handle.info().await;
    handle.shut_down().await;
}

#[cfg(all(
    feature = "reuseport",
    unix,
    not(target_os = "solaris"),
    not(target_os = "illumos"),
    not(target_os = "cygwin"),
    not(target_vendor = "apple")
))]
#[tokio::test]
async fn reuseport_serves_and_shuts_down() {
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use trillium::Conn;
    use trillium_tokio::server;

    let handle = server()
        .without_signals()
        .with_reuseport_workers(2)
        .bind_reuseport_tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap()
        .spawn(|conn: Conn| async move { conn.ok("hello reuseport") });

    let addr = *handle.info().await.tcp_socket_addr().unwrap();

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();

    assert!(response.contains("200 OK"), "{response}");
    assert!(response.contains("hello reuseport"), "{response}");

    handle.shut_down().await;
}

#[cfg(all(
    feature = "reuseport",
    unix,
    not(target_os = "solaris"),
    not(target_os = "illumos"),
    not(target_os = "cygwin"),
    not(target_vendor = "apple")
))]
#[tokio::test]
async fn reuseport_mixed_with_plain_tcp() {
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use trillium::Conn;
    use trillium_tokio::server;

    // A reuseport hot listener fanned across workers, alongside a plain admin listener on the
    // shared runtime. Both must serve: the plain listener's accept loop runs inline on the shared
    // runtime while the detached reuseport workers serve their own group.
    let handle = server()
        .without_signals()
        .with_reuseport_workers(2)
        .bind_tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap()
        .bind_reuseport_tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap()
        .spawn(|conn: Conn| async move { conn.ok("mixed") });

    // Plain listener is bound first, so it is the primary; the full set includes both.
    let addrs = handle.info().await.tcp_addrs().to_vec();
    assert_eq!(addrs.len(), 2, "{addrs:?}");

    for addr in addrs {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.contains("200 OK"), "{addr}: {response}");
        assert!(response.contains("mixed"), "{addr}: {response}");
    }

    handle.shut_down().await;
}
