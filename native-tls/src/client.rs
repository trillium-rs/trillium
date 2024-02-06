use async_native_tls::{TlsConnector, TlsStream};
use std::{
    fmt::{Debug, Formatter},
    future::Future,
    io::{Error, ErrorKind, IoSlice, IoSliceMut, Result},
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use trillium_server_common::{AsyncRead, AsyncWrite, Connector, Transport, Url};

/**
Configuration for the native tls client connector
*/
#[derive(Clone)]
pub struct NativeTlsConfig<Config> {
    /// configuration for the inner Connector (usually tcp)
    pub tcp_config: Config,

    /**
    native tls configuration

    Although async_native_tls calls this
    a TlsConnector, it's actually a builder ¯\_(ツ)_/¯
    */
    pub tls_connector: Arc<TlsConnector>,
}

impl<C: Connector> NativeTlsConfig<C> {
    /// replace the tcp config
    pub fn with_tcp_config(mut self, config: C) -> Self {
        self.tcp_config = config;
        self
    }
}

impl<C: Connector> From<C> for NativeTlsConfig<C> {
    fn from(tcp_config: C) -> Self {
        Self {
            tcp_config,
            tls_connector: Arc::new(TlsConnector::default()),
        }
    }
}

impl<Config: Debug> Debug for NativeTlsConfig<Config> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeTlsConfig")
            .field("tcp_config", &self.tcp_config)
            .field("tls_connector", &"..")
            .finish()
    }
}

impl<Config: Default> Default for NativeTlsConfig<Config> {
    fn default() -> Self {
        Self {
            tcp_config: Config::default(),
            tls_connector: Arc::new(TlsConnector::default()),
        }
    }
}

impl<Config> AsRef<Config> for NativeTlsConfig<Config> {
    fn as_ref(&self) -> &Config {
        &self.tcp_config
    }
}

impl<T: Connector> Connector for NativeTlsConfig<T> {
    type Transport = NativeTlsClientTransport<T::Transport>;

    async fn connect(&self, url: &Url) -> Result<Self::Transport> {
        match url.scheme() {
            "https" => {
                let mut http = url.clone();
                http.set_scheme("http").ok();
                http.set_port(url.port_or_known_default()).ok();
                let inner_stream = self.tcp_config.connect(&http).await?;

                self.tls_connector
                    .connect(url, inner_stream)
                    .await
                    .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
                    .map(NativeTlsClientTransport::from)
            }

            "http" => self
                .tcp_config
                .connect(url)
                .await
                .map(NativeTlsClientTransport::from),

            unknown => Err(Error::new(
                ErrorKind::InvalidInput,
                format!("unknown scheme {unknown}"),
            )),
        }
    }

    fn spawn<Fut: Future<Output = ()> + Send + 'static>(&self, fut: Fut) {
        self.tcp_config.spawn(fut)
    }

    async fn delay(&self, duration: std::time::Duration) {
        self.tcp_config.delay(duration).await
    }
}

/**
Client [`Transport`] for the native tls connector

This may represent either an encrypted tls connection or a plaintext
connection
*/

#[derive(Debug)]
pub struct NativeTlsClientTransport<T>(NativeTlsClientTransportInner<T>);

impl<T: AsyncWrite + AsyncRead + Unpin> NativeTlsClientTransport<T> {
    /// Borrow the TlsStream, if this connection is tls.
    ///
    /// Returns None otherwise
    pub fn as_tls(&self) -> Option<&TlsStream<T>> {
        match &self.0 {
            Tcp(_) => None,
            Tls(tls) => Some(tls),
        }
    }
}

impl<T> From<T> for NativeTlsClientTransport<T> {
    fn from(value: T) -> Self {
        Self(Tcp(value))
    }
}

impl<T> From<TlsStream<T>> for NativeTlsClientTransport<T> {
    fn from(value: TlsStream<T>) -> Self {
        Self(Tls(value))
    }
}

impl<T: Transport> AsRef<T> for NativeTlsClientTransport<T> {
    fn as_ref(&self) -> &T {
        match &self.0 {
            Tcp(transport) => transport,
            Tls(tls_stream) => tls_stream.get_ref(),
        }
    }
}

#[derive(Debug)]
enum NativeTlsClientTransportInner<T> {
    Tcp(T),
    Tls(TlsStream<T>),
}
use NativeTlsClientTransportInner::{Tcp, Tls};

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncRead for NativeTlsClientTransport<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Tcp(t) => Pin::new(t).poll_read(cx, buf),
            Tls(t) => Pin::new(t).poll_read(cx, buf),
        }
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Tcp(t) => Pin::new(t).poll_read_vectored(cx, bufs),
            Tls(t) => Pin::new(t).poll_read_vectored(cx, bufs),
        }
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncWrite for NativeTlsClientTransport<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Tcp(t) => Pin::new(t).poll_write(cx, buf),
            Tls(t) => Pin::new(t).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut self.0 {
            Tcp(t) => Pin::new(t).poll_flush(cx),
            Tls(t) => Pin::new(t).poll_flush(cx),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut self.0 {
            Tcp(t) => Pin::new(t).poll_close(cx),
            Tls(t) => Pin::new(t).poll_close(cx),
        }
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Tcp(t) => Pin::new(t).poll_write_vectored(cx, bufs),
            Tls(t) => Pin::new(t).poll_write_vectored(cx, bufs),
        }
    }
}

impl<T: Transport> Transport for NativeTlsClientTransport<T> {
    fn peer_addr(&self) -> Result<Option<SocketAddr>> {
        self.as_ref().peer_addr()
    }
}
