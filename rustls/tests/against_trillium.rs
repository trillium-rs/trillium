use std::{error::Error, future::Future};
use test_harness::test;
use trillium_client::Client;
use trillium_native_tls::{NativeTlsAcceptor, NativeTlsConfig};
use trillium_rustls::{RustlsAcceptor, RustlsConfig};
use trillium_server_common::Url;
use trillium_testing::{block_on, client_config, config};

fn handler() -> impl trillium::Handler {
    "ok"
}

pub fn with_native_tls_server<Fun, Fut>(tests: Fun)
where
    Fun: FnOnce(Url) -> Fut,
    Fut: Future<Output = Result<(), Box<dyn Error>>>,
{
    block_on(async move {
        let port = portpicker::pick_unused_port().expect("could not pick a port");
        let url = format!("https://localhost:{port}").parse().unwrap();
        let handle = config()
            .with_host("localhost")
            .with_port(port)
            .with_acceptor(NativeTlsAcceptor::from_pkcs12(
                include_bytes!("./identity.p12"),
                "changeit",
            ))
            .spawn(handler());
        handle.info().await;
        tests(url).await.unwrap();
        handle.stop().await;
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
        let handle = config()
            .with_host("localhost")
            .with_port(port)
            .with_acceptor(RustlsAcceptor::from_single_cert(
                include_bytes!("./cert.pem"),
                include_bytes!("./key.pem"),
            ))
            .spawn(handler());
        handle.info().await;
        tests(url).await.unwrap();
        handle.stop().await;
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
