use std::{env::var, error::Error, fs::read, future::Future};
use test_harness::test;
use trillium::Conn;
use trillium_client::{Client, Version};
use trillium_native_tls::{NativeTlsAcceptor, NativeTlsConfig};
use trillium_rustls::{RustlsAcceptor, RustlsConfig};
use trillium_server_common::Url;
use trillium_testing::{block_on, client_config, config, harness};

fn handler() -> impl trillium::Handler {
    "ok"
}

async fn report_secure(conn: Conn) -> Conn {
    let secure = conn.is_secure();
    conn.ok(secure.to_string())
}

fn pem_and_key() -> Option<(Vec<u8>, Vec<u8>)> {
    let root = var("CARGO_MANIFEST_DIR").ok()?;
    Some((
        read(format!("{root}/../localhost.pem")).ok()?,
        read(format!("{root}/../localhost-key.pem")).ok()?,
    ))
}

pub fn with_native_tls_server<Fun, Fut>(tests: Fun)
where
    Fun: FnOnce(Url) -> Fut,
    Fut: Future<Output = Result<(), Box<dyn Error>>>,
{
    block_on(async move {
        let port = portpicker::pick_unused_port().expect("could not pick a port");
        let url = format!("https://localhost:{port}").parse().unwrap();
        let Some((pem, key)) = pem_and_key() else {
            return;
        };

        let handle = config()
            .with_host("localhost")
            .with_port(port)
            .with_acceptor(NativeTlsAcceptor::from_pkcs8(&pem, &key))
            .spawn(handler());
        handle.info().await;
        tests(url).await.unwrap();
        handle.shut_down().await;
    });
}

pub fn with_rustls_server<Fun, Fut>(tests: Fun)
where
    Fun: FnOnce(Url) -> Fut,
    Fut: Future<Output = Result<(), Box<dyn Error>>>,
{
    block_on(async move {
        let port = portpicker::pick_unused_port().expect("could not pick a port");
        let url = format!("https://localhost:{port}").parse().unwrap();
        let Some((pem, key)) = pem_and_key() else {
            return;
        };

        let handle = config()
            .with_host("localhost")
            .with_port(port)
            .with_acceptor(RustlsAcceptor::from_single_cert(&pem, &key))
            .spawn(handler());
        handle.info().await;
        tests(url).await.unwrap();
        handle.shut_down().await;
    });
}

pub fn rustls_client() -> Client {
    Client::new(RustlsConfig {
        rustls_config: Default::default(),
        tcp_config: client_config(),
    })
}

pub fn native_tls_client() -> Client {
    Client::new(NativeTlsConfig {
        tcp_config: client_config(),
        tls_connector: Default::default(),
    })
}

#[test(harness = with_native_tls_server)]
async fn rustls_client_native_tls_server(url: Url) -> Result<(), Box<dyn Error>> {
    let _ = rustls_client().get(url).await?.success()?;
    Ok(())
}

#[test(harness = with_rustls_server)]
async fn rustls_client_rustls_server(url: Url) -> Result<(), Box<dyn Error>> {
    let _ = rustls_client().get(url).await?.success()?;
    Ok(())
}

#[test(harness = with_rustls_server)]
async fn native_tls_client_rustls_server(url: Url) -> Result<(), Box<dyn Error>> {
    let _ = native_tls_client().get(url).await?.success()?;
    Ok(())
}

#[test(harness)]
async fn h1_over_rustls_reports_conn_secure() -> Result<(), Box<dyn Error>> {
    // An HTTP/1.1 request over direct TLS must report `is_secure()` — it governs the `Secure`
    // cookie attribute and URL-scheme derivation. The h1 dispatch path historically never
    // stamped it from the acceptor the way h2/h3 do. Pin the client to HTTP/1.1 so the server
    // takes the h1 dispatch branch rather than negotiating h2 via ALPN.
    let Some((pem, key)) = pem_and_key() else {
        return Ok(());
    };
    let port = portpicker::pick_unused_port().expect("could not pick a port");
    let url: Url = format!("https://localhost:{port}").parse()?;

    let handle = config()
        .with_host("localhost")
        .with_port(port)
        .with_acceptor(RustlsAcceptor::from_single_cert(&pem, &key))
        .spawn(report_secure);
    handle.info().await;

    let mut conn = rustls_client()
        .get(url)
        .with_http_version(Version::Http1_1)
        .await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http1_1);
    assert_eq!(conn.response_body().read_string().await?, "true");

    handle.shut_down().await;
    Ok(())
}
