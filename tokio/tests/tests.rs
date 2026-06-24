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

#[cfg(unix)]
#[tokio::test]
async fn unix_socket_client_round_trip() {
    use trillium_client::Client;
    use trillium_tokio::UnixClientConfig;

    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("socket");

    let handle = config()
        .without_signals()
        .with_host(path.to_str().unwrap())
        .spawn("ok");
    handle.info().await;

    let client = Client::new(UnixClientConfig::new(path));
    let mut conn = client.get("http://localhost/").await.unwrap();

    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.response_body().read_string().await.unwrap(), "ok");
    handle.shut_down().await;
}

// Multi-listener `ListenerConfig` behavior is tested runtime-agnostically in
// `trillium-server-common` (over smol). The reuseport tests below stay here because they exercise
// `trillium-tokio`'s own `FanOut` impl, which is what enables `bind_reuseport_*`.

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
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
    };
    use trillium::Conn;

    let handle = config()
        .without_signals()
        .listeners()
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
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
    };
    use trillium::Conn;

    // A reuseport hot listener fanned across workers, alongside a plain admin listener on the
    // shared runtime. Both must serve: the plain listener's accept loop runs inline on the shared
    // runtime while the detached reuseport workers serve their own group.
    let handle = config()
        .without_signals()
        .listeners()
        .with_reuseport_workers(2)
        .bind_tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap()
        .bind_reuseport_tcp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .unwrap()
        .spawn(|conn: Conn| async move { conn.ok("mixed") });

    // Plain listener is bound first, so it is the primary; the full set includes both.
    let addrs = handle
        .info()
        .await
        .listeners()
        .iter()
        .filter_map(trillium::Listener::socket_addr)
        .collect::<Vec<_>>();
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
