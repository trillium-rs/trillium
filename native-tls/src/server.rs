use crate::Identity;
use async_native_tls::{Error, TlsAcceptor, TlsStream};
use std::{
    io::{self, IoSlice, IoSliceMut},
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use trillium_server_common::{Acceptor, AsyncRead, AsyncWrite, Transport};

/// trillium [`Acceptor`] for native-tls

#[derive(Clone, Debug)]
pub struct NativeTlsAcceptor(TlsAcceptor);

impl NativeTlsAcceptor {
    /// constructs a NativeTlsAcceptor from a [`native_tls::TlsAcceptor`],
    /// an [`async_native_tls::TlsAcceptor`], or an [`Identity`]
    pub fn new(t: impl Into<Self>) -> Self {
        t.into()
    }

    /// constructs a NativeTlsAcceptor from a pkcs12 key and password. See
    /// See [`Identity::from_pkcs8`]
    pub fn from_pkcs12(der: &[u8], password: &str) -> Self {
        Identity::from_pkcs12(der, password)
            .expect("could not build Identity from provided pkcs12 key and password")
            .into()
    }

    /// constructs a NativeTlsAcceptor from a pkcs8 pem and private
    /// key. See [`Identity::from_pkcs8`]
    pub fn from_pkcs8(pem: &[u8], key: &[u8]) -> Self {
        Identity::from_pkcs8(pem, key)
            .expect("could not build Identity from provided pem and key")
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

impl<Input> Acceptor<Input> for NativeTlsAcceptor
where
    Input: Transport,
{
    type Error = Error;
    type Output = NativeTlsServerTransport<Input>;

    async fn accept(&self, input: Input) -> Result<Self::Output, Self::Error> {
        self.0.accept(input).await.map(NativeTlsServerTransport)
    }
}

/// Server Tls Transport
///
/// A wrapper type around [`TlsStream`] that also implements [`Transport`]
#[derive(Debug)]
pub struct NativeTlsServerTransport<T>(TlsStream<T>);

impl<T: AsyncWrite + AsyncRead + Unpin> AsRef<T> for NativeTlsServerTransport<T> {
    fn as_ref(&self) -> &T {
        self.0.get_ref()
    }
}
impl<T: AsyncWrite + AsyncRead + Unpin> AsMut<T> for NativeTlsServerTransport<T> {
    fn as_mut(&mut self) -> &mut T {
        self.0.get_mut()
    }
}

impl<T> AsRef<TlsStream<T>> for NativeTlsServerTransport<T> {
    fn as_ref(&self) -> &TlsStream<T> {
        &self.0
    }
}
impl<T> AsMut<TlsStream<T>> for NativeTlsServerTransport<T> {
    fn as_mut(&mut self) -> &mut TlsStream<T> {
        &mut self.0
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncRead for NativeTlsServerTransport<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_read_vectored(cx, bufs)
    }
}

impl<T: AsyncWrite + AsyncRead + Unpin> AsyncWrite for NativeTlsServerTransport<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_close(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write_vectored(cx, bufs)
    }
}

impl<T: Transport> Transport for NativeTlsServerTransport<T> {
    fn peer_addr(&self) -> io::Result<Option<SocketAddr>> {
        self.0.get_ref().peer_addr()
    }
}
