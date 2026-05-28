//! Tests for the multi-listener [`ListenerConfig`], the [`Config::listeners`] bridge, and
//! [`ListenerConfig::bind_server`]. These exercise runtime-agnostic `trillium-server-common` logic
//! using `trillium-smol` as a concrete runtime, so they run wherever the smol suite does rather
//! than being pinned to one runtime adapter.

use futures_lite::{AsyncReadExt, AsyncWriteExt};
use std::net::{Ipv4Addr, SocketAddr};
use trillium::{Conn, Headers, KnownHeaderName, ListenerKind};
use trillium_smol::{
    async_global_executor::block_on,
    async_net::{TcpListener, TcpStream},
    config,
};

fn localhost() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
}

/// One blocking GET against `addr`, returning the raw response text.
async fn get(addr: SocketAddr) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
}

fn has_server_header(info: &trillium_server_common::BoundInfo) -> bool {
    info.shared_state::<Headers>()
        .is_some_and(|h| h.get_str(KnownHeaderName::Server).is_some())
}

#[test]
fn multi_carries_global_config() {
    block_on(async {
        let handle = config()
            .without_signals()
            .with_shared_state(Marker)
            .listeners()
            .bind_tcp(localhost())
            .unwrap()
            .spawn(|conn: Conn| async move { conn.ok("hi") });

        let info = handle.info().await;
        assert!(
            info.shared_state::<Marker>().is_some(),
            "shared state carried"
        );
        assert!(has_server_header(&info), "server header set");
        assert_eq!(info.listeners().len(), 1);
        assert!(matches!(info.listeners()[0].kind(), ListenerKind::Tcp(_)));
        assert!(!info.listeners()[0].is_secure());
        assert!(info.url().is_some(), "url derived from primary");

        handle.shut_down().await;
    });
}

#[test]
fn multi_binds_multiple_listeners() {
    block_on(async {
        let handle = config()
            .without_signals()
            .listeners()
            .bind_tcp(localhost())
            .unwrap()
            .bind_tcp(localhost())
            .unwrap()
            .spawn(|conn: Conn| async move { conn.ok("multi") });

        let info = handle.info().await;
        let addrs: Vec<_> = info
            .listeners()
            .iter()
            .filter_map(|l| l.socket_addr())
            .collect();
        assert_eq!(addrs.len(), 2, "{addrs:?}");
        assert_ne!(addrs[0], addrs[1]);

        for addr in addrs {
            let response = get(addr).await;
            assert!(response.contains("200 OK"), "addr {addr}: {response}");
            assert!(response.contains("multi"), "addr {addr}: {response}");
        }

        handle.shut_down().await;
    });
}

#[test]
fn bind_server_serves_and_enumerates() {
    block_on(async {
        let listener = TcpListener::bind(localhost()).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = config()
            .without_signals()
            .listeners()
            .bind_server(listener)
            .spawn(|conn: Conn| async move { conn.ok("adopted") });

        let info = handle.info().await;
        // The adopted server's record is reconstructed from its post-init address + acceptor, so it
        // enumerates like any other listener despite the builder never having held a std listener.
        assert_eq!(info.listeners().len(), 1, "{:?}", info.listeners());
        assert_eq!(info.listeners()[0].socket_addr(), Some(addr));
        assert!(!info.listeners()[0].is_secure());
        assert_eq!(info.tcp_socket_addr(), Some(&addr));

        let response = get(addr).await;
        assert!(response.contains("200 OK"), "{response}");
        assert!(response.contains("adopted"), "{response}");

        handle.shut_down().await;
    });
}

/// The single-listener [`Config`] default-bind path that the `listeners()` collapse must preserve:
/// one tcp listener, a derived url, the server header, and carried shared state.
#[test]
fn config_default_path() {
    block_on(async {
        let handle = config()
            .without_signals()
            .with_port(0)
            .with_shared_state(Marker)
            .spawn(|conn: Conn| async move { conn.ok("config") });

        let info = handle.info().await;
        assert!(info.shared_state::<Marker>().is_some());
        assert!(has_server_header(&info));
        assert_eq!(info.listeners().len(), 1);
        assert!(matches!(info.listeners()[0].kind(), ListenerKind::Tcp(_)));
        assert!(info.url().is_some());

        let addr = *info.tcp_socket_addr().unwrap();
        let response = get(addr).await;
        assert!(response.contains("200 OK"), "{response}");
        assert!(response.contains("config"), "{response}");

        handle.shut_down().await;
    });
}

/// The single-listener [`Config::with_prebound_server`] path. After the collapse this routes
/// through [`ListenerConfig::bind_server`], so this asserts the bridge stays faithful to it.
#[test]
fn config_prebound_server_path() {
    block_on(async {
        let listener = TcpListener::bind(localhost()).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = config()
            .without_signals()
            .with_prebound_server(listener)
            .spawn(|conn: Conn| async move { conn.ok("prebound") });

        let info = handle.info().await;
        assert_eq!(info.listeners().len(), 1, "{:?}", info.listeners());
        assert_eq!(info.tcp_socket_addr(), Some(&addr));

        let response = get(addr).await;
        assert!(response.contains("200 OK"), "{response}");
        assert!(response.contains("prebound"), "{response}");

        handle.shut_down().await;
    });
}

/// Two distinct concrete acceptor types must coexist in one builder (exercising the boxed-acceptor
/// erasure). `Passthrough` passes the transport through unchanged but reports itself secure, so
/// both listeners still speak plaintext HTTP for the round-trip.
#[test]
fn multi_mixed_acceptors() {
    use std::convert::Infallible;
    use trillium_server_common::{Acceptor, Transport};

    #[derive(Clone)]
    struct Passthrough;
    impl<T: Transport> Acceptor<T> for Passthrough {
        type Error = Infallible;
        type Output = T;

        async fn accept(&self, input: T) -> Result<T, Infallible> {
            Ok(input)
        }

        fn is_secure(&self) -> bool {
            true
        }
    }

    block_on(async {
        let handle = config()
            .without_signals()
            .listeners()
            .bind_tcp(localhost())
            .unwrap()
            .bind_tls(localhost(), Passthrough)
            .unwrap()
            .spawn(|conn: Conn| async move { conn.ok("mixed") });

        let info = handle.info().await;
        let addrs: Vec<_> = info
            .listeners()
            .iter()
            .filter_map(|l| l.socket_addr())
            .collect();
        assert_eq!(addrs.len(), 2, "{addrs:?}");
        assert!(
            info.listeners()[0].kind().eq(&ListenerKind::Tcp(addrs[0]))
                && !info.listeners()[0].is_secure()
        );
        assert!(
            info.listeners()[1].is_secure(),
            "tls listener reports secure"
        );

        for addr in addrs {
            let response = get(addr).await;
            assert!(response.contains("200 OK"), "addr {addr}: {response}");
            assert!(response.contains("mixed"), "addr {addr}: {response}");
        }

        handle.shut_down().await;
    });
}

/// The originating listener is stamped into each conn's state as both a `SocketAddr` (ingress
/// address) and a [`Listener`](trillium::Listener) (full provenance); the two must agree.
#[test]
fn multi_ingress_identity() {
    use trillium::Listener;

    async fn handler(conn: Conn) -> Conn {
        let addr = conn.state::<SocketAddr>().copied();
        let listener_addr = conn.state::<Listener>().and_then(Listener::socket_addr);
        let body = match (addr, listener_addr) {
            (Some(addr), Some(listener_addr)) if addr == listener_addr => format!("local={addr}"),
            other => format!("mismatch={other:?}"),
        };
        conn.ok(body)
    }

    block_on(async {
        let handle = config()
            .without_signals()
            .listeners()
            .bind_tcp(localhost())
            .unwrap()
            .bind_tcp(localhost())
            .unwrap()
            .spawn(handler);

        let info = handle.info().await;
        let addrs: Vec<_> = info
            .listeners()
            .iter()
            .filter_map(|l| l.socket_addr())
            .collect();
        assert_eq!(addrs.len(), 2, "{addrs:?}");

        for addr in addrs {
            let response = get(addr).await;
            assert!(
                response.contains(&format!("local={addr}")),
                "conn on {addr} should report its ingress addr: {response}"
            );
        }

        handle.shut_down().await;
    });
}

/// A same-port `with_alt_svc(from, to)` declaration attaches `alt-svc: h3=":<to>"` to that
/// listener's responses.
#[test]
fn multi_alt_svc() {
    use std::net::TcpListener as StdTcpListener;

    block_on(async {
        // Claim a free port, then drop the std listener so the builder can re-bind it. Brief TOCTOU
        // window, but the alternative is a fixed port that risks CI collision.
        let port = StdTcpListener::bind(localhost())
            .unwrap()
            .local_addr()
            .unwrap()
            .port();

        let handle = config()
            .without_signals()
            .listeners()
            .bind_tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
            .unwrap()
            .with_alt_svc(port, 4433)
            .spawn(|conn: Conn| async move { conn.ok("alt-svc") });

        let addr = *handle.info().await.tcp_socket_addr().unwrap();
        let response = get(addr).await;
        assert!(response.contains("200 OK"), "{response}");
        assert!(
            response
                .to_ascii_lowercase()
                .contains("alt-svc: h3=\":4433\""),
            "expected alt-svc header pointing at port 4433: {response}"
        );

        handle.shut_down().await;
    });
}

/// The `bind_quic` path: claim the UDP socket at bind time, construct the endpoint inside the
/// runtime, and spawn the h3 accept loop — exercised with a no-op `QuicConfig` so no real QUIC
/// stack is needed.
#[test]
fn multi_bind_quic() {
    use std::{io, net::UdpSocket as StdUdpSocket};
    use trillium::Info;
    use trillium_server_common::{QuicConfig, Server};

    struct MockQuic;
    impl<S: Server> QuicConfig<S> for MockQuic {
        type Endpoint = ();

        fn bind(self, _: SocketAddr, _: S::Runtime, _: &mut Info) -> Option<io::Result<()>> {
            Some(Ok(()))
        }

        fn bind_with_socket(self, _: StdUdpSocket, _: S::Runtime, _: &mut Info) -> io::Result<()> {
            Ok(())
        }
    }

    block_on(async {
        let handle = config()
            .without_signals()
            .listeners()
            .bind_tcp(localhost())
            .unwrap()
            .bind_quic(localhost(), MockQuic)
            .unwrap()
            .spawn(|conn: Conn| async move { conn.ok("ok") });

        // Reaching this point without panicking means the UDP socket was claimed, the endpoint
        // built inside the runtime, and the h3 accept loop spawned.
        let _info = handle.info().await;
        handle.shut_down().await;
    });
}

/// A Unix-domain-socket listener serves and enumerates with its path.
#[cfg(unix)]
#[test]
fn multi_bind_uds() {
    use futures_lite::{AsyncReadExt, AsyncWriteExt};
    use trillium::ListenerKind;
    use trillium_smol::async_net::unix::UnixStream;

    block_on(async {
        let path = std::env::temp_dir().join(format!("trillium-uds-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let handle = config()
            .without_signals()
            .listeners()
            .bind_uds(&path)
            .unwrap()
            .spawn(|conn: Conn| async move { conn.ok("hello uds") });

        let info = handle.info().await;
        let listeners = info.listeners();
        assert_eq!(listeners.len(), 1, "{listeners:?}");
        assert!(
            matches!(listeners[0].kind(), ListenerKind::Unix(Some(p)) if p == &path),
            "{:?}",
            listeners[0].kind()
        );

        let mut stream = UnixStream::connect(&path).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.contains("200 OK"), "{response}");
        assert!(response.contains("hello uds"), "{response}");

        handle.shut_down().await;
        let _ = std::fs::remove_file(&path);
    });
}

#[derive(Clone, Copy, Debug)]
struct Marker;
