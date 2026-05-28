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

#[cfg(all(
    feature = "reuseport",
    unix,
    not(target_os = "solaris"),
    not(target_os = "illumos"),
    not(target_os = "cygwin"),
    not(target_vendor = "apple")
))]
#[test]
fn reuseport_serves_and_shuts_down() {
    use std::{
        io::{Read, Write},
        net::TcpStream,
    };
    use trillium::Conn;
    use trillium_tokio::ReuseportConfigExt;

    let handle = config()
        .with_host("127.0.0.1")
        .with_port(0)
        .without_signals()
        .spawn_reuseport(|conn: Conn| async move { conn.ok("hello reuseport") });

    let mut stream = TcpStream::connect(handle.local_addr()).unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();

    assert!(response.contains("200 OK"), "{response}");
    assert!(response.contains("hello reuseport"), "{response}");

    handle.shut_down();
    handle.block();
}
