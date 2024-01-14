use crate::RustlsTransport;
use futures_rustls::TlsAcceptor;
use rustls::ServerConfig;
use std::{
    fmt::{Debug, Formatter},
    io,
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
    #[cfg(any(feature = "ring", feature = "aws-lc-rs"))]
    pub fn from_single_cert(cert: &[u8], key: &[u8]) -> Self {
        use std::io::Cursor;

        let mut br = Cursor::new(cert);
        let certs = rustls_pemfile::certs(&mut br)
            .collect::<Result<Vec<_>, _>>()
            .expect("could not read certificate");

        let mut br = Cursor::new(key);
        let key = rustls_pemfile::pkcs8_private_keys(&mut br)
            .next()
            .expect("no pkcs8 private key found in `key`")
            .expect("could not read key pemfile");

        #[cfg(all(feature = "ring", not(feature = "aws-lc-rs")))]
        let provider = rustls::crypto::ring::default_provider();
        #[cfg(feature = "aws-lc-rs")]
        let provider = rustls::crypto::aws_lc_rs::default_provider();

        ServerConfig::builder_with_provider(Arc::new(provider))
            .with_safe_default_protocol_versions()
            .expect("could not enable default TLS versions")
            .with_no_client_auth()
            .with_single_cert(certs, key.into())
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
