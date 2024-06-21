use crate::crypto_provider;
use futures_rustls::{
    rustls::{ServerConfig, ServerConnection},
    server::TlsStream,
    TlsAcceptor,
};
use std::{
    fmt::{Debug, Formatter},
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use trillium_server_common::{Acceptor, AsyncRead, AsyncWrite, Transport};

/**
trillium [`Acceptor`] for Rustls
*/

#[derive(Clone)]
pub struct RustlsAcceptor(TlsAcceptor);
impl Debug for RustlsAcceptor {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Rustls").field(&"<<TlsAcceptor>>").finish()
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
    build a new RustlsAcceptor from a cert chain (pem) and private key.

    See
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
    pub fn from_single_cert(cert: &[u8], key: &[u8]) -> Self {
        use std::io::Cursor;

        let cert_chain = rustls_pemfile::certs(&mut Cursor::new(cert))
            .collect::<Result<_, _>>()
            .expect("could not read certificate");

        let key_der = rustls_pemfile::private_key(&mut Cursor::new(key))
            .expect("could not read key pemfile")
            .expect("no private key found in `key`");

        ServerConfig::builder_with_provider(crypto_provider())
            .with_safe_default_protocol_versions()
            .expect("crypto provider did not support safe default protocol versions")
            .with_no_client_auth()
            .with_single_cert(cert_chain, key_der)
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

/// Transport for rustls server acceptor
#[derive(Debug)]
pub struct RustlsServerTransport<T>(TlsStream<T>);

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncRead for RustlsServerTransport<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + AsyncRead + Unpin> AsyncWrite for RustlsServerTransport<T> {
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
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write_vectored(cx, bufs)
    }
}

impl<T: Transport> Transport for RustlsServerTransport<T> {
    fn peer_addr(&self) -> io::Result<Option<std::net::SocketAddr>> {
        self.inner_transport().peer_addr()
    }
}

impl<T> RustlsServerTransport<T> {
    /// access the contained transport type (eg TcpStream)
    pub fn inner_transport(&self) -> &T {
        self.0.get_ref().0
    }

    /// mutably access the contained transport type (eg TcpStream)
    pub fn inner_transport_mut(&mut self) -> &mut T {
        self.0.get_mut().0
    }
}

impl<T> AsRef<ServerConnection> for RustlsServerTransport<T> {
    fn as_ref(&self) -> &ServerConnection {
        self.0.get_ref().1
    }
}

impl<T> AsMut<ServerConnection> for RustlsServerTransport<T> {
    fn as_mut(&mut self) -> &mut ServerConnection {
        self.0.get_mut().1
    }
}

impl<T> From<TlsStream<T>> for RustlsServerTransport<T> {
    fn from(value: TlsStream<T>) -> Self {
        Self(value)
    }
}

impl<T> From<RustlsServerTransport<T>> for TlsStream<T> {
    fn from(RustlsServerTransport(value): RustlsServerTransport<T>) -> Self {
        value
    }
}

impl<Input> Acceptor<Input> for RustlsAcceptor
where
    Input: Transport,
{
    type Error = io::Error;
    type Output = RustlsServerTransport<Input>;

    async fn accept(&self, input: Input) -> Result<Self::Output, Self::Error> {
        self.0.accept(input).await.map(RustlsServerTransport)
    }
}
