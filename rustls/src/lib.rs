#![forbid(unsafe_code)]
#![warn(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

pub use async_tls;
use async_tls::server::TlsStream;
use async_tls::TlsAcceptor;
pub use rustls;
use rustls::internal::pemfile::{certs, pkcs8_private_keys};
use rustls::{NoClientAuth, ServerConfig};
use std::io::BufReader;
use trillium_tls_common::{async_trait, Acceptor, AsyncRead, AsyncWrite};

#[derive(Clone)]
pub struct RustTls(TlsAcceptor);
impl std::fmt::Debug for RustTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("RustTls").field(&"<<TlsAcceptor>>").finish()
    }
}
impl RustTls {
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

impl From<ServerConfig> for RustTls {
    fn from(sc: ServerConfig) -> Self {
        Self(sc.into())
    }
}

impl From<TlsAcceptor> for RustTls {
    fn from(ta: TlsAcceptor) -> Self {
        Self(ta)
    }
}

#[async_trait]
impl<Input> Acceptor<Input> for RustTls
where
    Input: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    type Output = TlsStream<Input>;
    type Error = std::io::Error;
    async fn accept(&self, input: Input) -> Result<Self::Output, Self::Error> {
        self.0.accept(input).await
    }
}
