use crate::RustlsTransport;
use async_rustls::TlsAcceptor;
use rustls::{Certificate, PrivateKey, ServerConfig};
use rustls_pemfile::certs;
use std::{
    fmt::{Debug, Formatter},
    io::{self, BufReader},
    sync::Arc,
};
use trillium_server_common::{async_trait, Acceptor, Transport};

/**
trillium [`Acceptor`] for Rustls
*/

#[derive(Clone)]
pub struct RustlsAcceptor(TlsAcceptor);
impl Debug for RustlsAcceptor {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("RustTls").field(&"<<TlsAcceptor>>").finish()
    }
}

impl RustlsAcceptor {
    /**
    build a new RustlsAcceptor from a [`ServerConfig`] or a [`TlsAcceptor`]
    */
    pub fn new(t: impl Into<Self>) -> Self {
        t.into()
    }

    /**
    build a new RustlsAcceptor from a cert chain and key. See
    [`ConfigBuilder::with_single_cert`][`crate::rustls::ConfigBuilder::with_single_cert`]
    for accepted formats. If you need to customize the
    [`ServerConfig`], use ServerConfig's Into RustlsAcceptor, eg

    ```rust,ignore
    use trillium_rustls::{rustls::ServerConfig, RustlsAcceptor};
    let rustls_acceptor: RustlsAcceptor = ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(certs, private_key)
        .expect("could not build rustls ServerConfig")
        .into();
    ```

    # Example

    ```rust,no_run
    use trillium_rustls::RustlsAcceptor;
    const KEY: &[u8] = include_bytes!("../examples/key.pem");
    const CERT: &[u8] = include_bytes!("../examples/cert.pem");
    let rustls_acceptor = RustlsAcceptor::from_single_cert(CERT, KEY);
    ```
    */
    pub fn from_single_cert(cert: &[u8], key: &[u8]) -> Self {
        let mut br = BufReader::new(cert);
        let certs = certs(&mut br)
            .expect("could not read cert pemfile")
            .into_iter()
            .map(Certificate)
            .collect();

        let mut br = BufReader::new(key);
        let key = rustls_pemfile::pkcs8_private_keys(&mut br)
            .expect("could not read key pemfile")
            .first()
            .expect("no pkcs8 private key found in `key`")
            .to_owned();

        ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(certs, PrivateKey(key))
            .expect("could not create a rustls ServerConfig from the supplied cert and key")
            .into()
    }
}

impl From<ServerConfig> for RustlsAcceptor {
    fn from(sc: ServerConfig) -> Self {
        Self(Arc::new(sc).into())
    }
}

impl From<TlsAcceptor> for RustlsAcceptor {
    fn from(ta: TlsAcceptor) -> Self {
        Self(ta)
    }
}

#[async_trait]
impl<Input> Acceptor<Input> for RustlsAcceptor
where
    Input: Transport,
{
    type Output = RustlsTransport<Input>;
    type Error = io::Error;
    async fn accept(&self, input: Input) -> Result<Self::Output, Self::Error> {
        self.0.accept(input).await.map(RustlsTransport::from)
    }
}
