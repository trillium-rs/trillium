use rcgen::generate_simple_self_signed;
use std::{error::Error, future::Future};
use test_harness::test;
use trillium_client::Client;
use trillium_rustls::{RustlsAcceptor, RustlsClientConfig, RustlsConfig};
use trillium_server_common::Url;
use trillium_testing::{block_on, client_config, config, harness};

fn handler() -> impl trillium::Handler {
    "ok"
}

struct SelfSigned {
    cert_pem: Vec<u8>,
    key_pem: Vec<u8>,
}

fn self_signed() -> SelfSigned {
    let cert = generate_simple_self_signed(["localhost".to_string()]).unwrap();
    SelfSigned {
        cert_pem: cert.cert.pem().into_bytes(),
        key_pem: cert.signing_key.serialize_pem().into_bytes(),
    }
}

fn client_with(rustls_config: RustlsClientConfig) -> Client {
    Client::new(RustlsConfig {
        rustls_config,
        tcp_config: client_config(),
    })
}

/// Spawns a server presenting a freshly generated self-signed `localhost` certificate and hands
/// the test both the URL and the certificate PEM.
fn with_self_signed_server<Fun, Fut>(tests: Fun)
where
    Fun: FnOnce(Url, Vec<u8>) -> Fut,
    Fut: Future<Output = Result<(), Box<dyn Error>>>,
{
    block_on(async move {
        let SelfSigned { cert_pem, key_pem } = self_signed();
        let port = portpicker::pick_unused_port().expect("could not pick a port");
        let url = format!("https://localhost:{port}").parse().unwrap();

        let handle = config()
            .with_host("localhost")
            .with_port(port)
            .with_acceptor(RustlsAcceptor::from_single_cert(&cert_pem, &key_pem))
            .spawn(handler());
        handle.info().await;
        tests(url, cert_pem).await.unwrap();
        handle.shut_down().await;
    });
}

#[test(harness = with_self_signed_server)]
async fn from_root_cert_pem_trusts_the_cert(
    url: Url,
    cert_pem: Vec<u8>,
) -> Result<(), Box<dyn Error>> {
    let config = RustlsClientConfig::from_root_cert_pem(&cert_pem)?;
    let _ = client_with(config).get(url).await?.success()?;
    Ok(())
}

#[test(harness = with_self_signed_server)]
async fn default_client_rejects_self_signed(
    url: Url,
    _cert_pem: Vec<u8>,
) -> Result<(), Box<dyn Error>> {
    assert!(
        client_with(RustlsClientConfig::default())
            .get(url)
            .await
            .is_err(),
        "default client must not trust an untrusted self-signed certificate"
    );
    Ok(())
}

#[cfg(feature = "dangerous")]
#[test(harness = with_self_signed_server)]
async fn dangerously_accept_any_cert_connects(
    url: Url,
    _cert_pem: Vec<u8>,
) -> Result<(), Box<dyn Error>> {
    let _ = client_with(RustlsClientConfig::dangerously_accept_any_cert())
        .get(url)
        .await?
        .success()?;
    Ok(())
}

#[test(harness)]
async fn from_root_cert_pem_rejects_invalid_input() {
    assert!(RustlsClientConfig::from_root_cert_pem(b"").is_err());
    assert!(RustlsClientConfig::from_root_cert_pem(b"not a certificate").is_err());
}
