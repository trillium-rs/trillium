pub use async_native_tls;
use async_native_tls::{Error, TlsAcceptor, TlsStream};
use myco_tls_common::{async_trait, Acceptor, AsyncRead, AsyncWrite};
pub use native_tls;
pub use native_tls::Identity;

#[derive(Clone)]
pub struct NativeTls(TlsAcceptor);
impl NativeTls {
    pub fn new(t: impl Into<Self>) -> Self {
        t.into()
    }

    pub fn from_pkcs12(key: &[u8], password: &str) -> Self {
        Identity::from_pkcs12(key, password)
            .expect("could not build Identity from provided pkcs12 key and password")
            .into()
    }
}

impl From<native_tls::Identity> for NativeTls {
    fn from(i: native_tls::Identity) -> Self {
        native_tls::TlsAcceptor::new(i).unwrap().into()
    }
}

impl From<native_tls::TlsAcceptor> for NativeTls {
    fn from(i: native_tls::TlsAcceptor) -> Self {
        Self(i.into())
    }
}

impl From<async_native_tls::TlsAcceptor> for NativeTls {
    fn from(i: async_native_tls::TlsAcceptor) -> Self {
        Self(i)
    }
}

impl From<(&[u8], &str)> for NativeTls {
    fn from(i: (&[u8], &str)) -> Self {
        Self::from_pkcs12(i.0, i.1)
    }
}

#[async_trait]
impl<Input> Acceptor<Input> for NativeTls
where
    Input: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    type Output = TlsStream<Input>;
    type Error = Error;
    async fn accept(&self, input: Input) -> Result<Self::Output, Self::Error> {
        self.0.accept(input).await
    }
}
