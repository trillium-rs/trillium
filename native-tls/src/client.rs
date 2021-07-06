use async_native_tls::{TlsConnector, TlsStream};
use std::{
    fmt::Debug,
    future::Future,
    io::{Error, ErrorKind, Result},
    marker::PhantomData,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use trillium_tls_common::{async_trait, AsConnector, AsyncRead, AsyncWrite, Connector, Url};
use NativeTlsTransportInner::{Tcp, Tls};

/**
Transport for the native tls connector

This may represent either an encrypted tls connection or a plaintext
connection, depending on the request scheme.
*/

#[derive(Debug)]
pub struct NativeTlsTransport<T>(NativeTlsTransportInner<T>);

#[derive(Debug)]
enum NativeTlsTransportInner<T> {
    Tcp(T),
    Tls(TlsStream<T>),
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncRead for NativeTlsTransport<T> {
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
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncWrite for NativeTlsTransport<T> {
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
}
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

impl<Config: Debug> Debug for NativeTlsConfig<Config> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

/**
trillium client connector for native tls
*/
#[derive(Clone, Copy, Debug)]
pub struct NativeTlsConnector<T>(PhantomData<T>);

impl<C> AsConnector<C> for NativeTlsConnector<C>
where
    C: Connector,
{
    type Connector = Self;
}

#[async_trait]
impl<T: Connector> Connector for NativeTlsConnector<T> {
    type Config = NativeTlsConfig<T::Config>;
    type Transport = NativeTlsTransport<T::Transport>;

    fn peer_addr(transport: &Self::Transport) -> Result<SocketAddr> {
        match &transport.0 {
            Tcp(transport) => T::peer_addr(transport),
            Tls(tls_stream) => T::peer_addr(tls_stream.get_ref()),
        }
    }

    async fn connect(url: &Url, config: &Self::Config) -> Result<Self::Transport> {
        match url.scheme() {
            "https" => {
                let mut http = url.clone();
                http.set_scheme("http").ok();
                http.set_port(url.port_or_known_default()).ok();
                let inner_stream = T::connect(&http, config.as_ref()).await?;
                let tls_stream = config
                    .tls_connector
                    .connect(url, inner_stream)
                    .await
                    .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?;
                Ok(NativeTlsTransport(Tls(tls_stream)))
            }

            "http" => Ok(NativeTlsTransport(Tcp(
                T::connect(url, config.as_ref()).await?
            ))),

            unknown => Err(Error::new(
                ErrorKind::InvalidInput,
                format!("unknown scheme {}", unknown),
            )),
        }
    }

    fn spawn<Fut>(future: Fut)
    where
        Fut: Future + Send + 'static,
        <Fut as Future>::Output: Send,
    {
        T::spawn(future);
    }
}
