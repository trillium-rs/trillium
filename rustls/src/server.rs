use async_rustls::{server::TlsStream, TlsAcceptor, TlsConnector};
use rustls::{
    internal::pemfile::{certs, pkcs8_private_keys},
    NoClientAuth, ServerConfig,
};
use std::{
    fmt::{Debug, Formatter},
    io::{BufReader, Error, Result},
    sync::Arc,
};
use trillium_tls_common::{async_trait, Acceptor, AsyncRead, AsyncWrite};

#[derive(Clone)]
pub struct RustlsAcceptor(TlsAcceptor);
impl Debug for RustlsAcceptor {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("RustTls").field(&"<<TlsAcceptor>>").finish()
    }
}

impl RustlsAcceptor {
    pub fn new(t: impl Into<Self>) -> Self {
        t.into()
    }

    pub fn from_pkcs8(cert: &[u8], key: &[u8]) -> Self {
        let mut config = ServerConfig::new(NoClientAuth::new());

        config
            .set_single_cert(
                certs(&mut BufReader::new(cert)).unwrap(),
                pkcs8_private_keys(&mut BufReader::new(key))
                    .unwrap()
                    .remove(0),
            )
            .expect("could not create a rustls ServerConfig from the supplied cert and key");

        config.into()
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
    Input: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    type Output = TlsStream<Input>;
    type Error = Error;
    async fn accept(&self, input: Input) -> Result<Self::Output> {
        self.0.accept(input).await
    }
}
