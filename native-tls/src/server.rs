use crate::{Identity, NativeTlsTransport};
use async_native_tls::{Error, TlsAcceptor};
use trillium_server_common::{async_trait, Acceptor, Transport};

/**
trillium [`Acceptor`] for native-tls
*/

#[derive(Clone, Debug)]
pub struct NativeTlsAcceptor(TlsAcceptor);

impl NativeTlsAcceptor {
    /**
    constructs a NativeTlsAcceptor from a [`native_tls::TlsAcceptor`],
    an [`async_native_tls::TlsAcceptor`], or an [`Identity`]
    */
    pub fn new(t: impl Into<Self>) -> Self {
        t.into()
    }

    /**
    constructs a NativeTlsAcceptor from a pkcs12 key and password
    */
    pub fn from_pkcs12(der: &[u8], password: &str) -> Self {
        Identity::from_pkcs12(der, password)
            .expect("could not build Identity from provided pkcs12 key and password")
            .into()
    }
}

impl From<Identity> for NativeTlsAcceptor {
    fn from(i: Identity) -> Self {
        native_tls::TlsAcceptor::new(i).unwrap().into()
    }
}

impl From<native_tls::TlsAcceptor> for NativeTlsAcceptor {
    fn from(i: native_tls::TlsAcceptor) -> Self {
        Self(i.into())
    }
}

impl From<TlsAcceptor> for NativeTlsAcceptor {
    fn from(i: TlsAcceptor) -> Self {
        Self(i)
    }
}

impl From<(&[u8], &str)> for NativeTlsAcceptor {
    fn from(i: (&[u8], &str)) -> Self {
        Self::from_pkcs12(i.0, i.1)
    }
}

#[async_trait]
impl<Input> Acceptor<Input> for NativeTlsAcceptor
where
    Input: Transport,
{
    type Output = NativeTlsTransport<Input>;
    type Error = Error;
    async fn accept(&self, input: Input) -> Result<Self::Output, Self::Error> {
        self.0.accept(input).await.map(NativeTlsTransport::from)
    }
}
