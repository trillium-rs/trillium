use async_native_tls::TlsStream;
use std::{
    io::{Error, ErrorKind, Result},
    marker::PhantomData,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use trillium_tls_common::{async_trait, AsyncRead, AsyncWrite, Connector, Url};
use NativeTlsTransport::{Tcp, Tls};

#[derive(Debug)]
pub enum NativeTlsTransport<T> {
    Tcp(T),
    Tls(TlsStream<T>),
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncRead for NativeTlsTransport<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        match &mut *self {
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
        match &mut *self {
            Tcp(t) => Pin::new(t).poll_write(cx, buf),
            Tls(t) => Pin::new(t).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut *self {
            Tcp(t) => Pin::new(t).poll_flush(cx),
            Tls(t) => Pin::new(t).poll_flush(cx),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut *self {
            Tcp(t) => Pin::new(t).poll_close(cx),
            Tls(t) => Pin::new(t).poll_close(cx),
        }
    }
}

#[derive(Default, Debug, Clone, Copy)]
pub struct NativeTlsConfig<Config> {
    pub tcp_config: Config,
}

impl<Config> AsRef<Config> for NativeTlsConfig<Config> {
    fn as_ref(&self) -> &Config {
        &self.tcp_config
    }
}

#[derive(Clone, Copy, Debug)]
pub struct NativeTlsConnector<T>(PhantomData<T>);

#[async_trait]
impl<T: Connector> Connector for NativeTlsConnector<T> {
    type Config = NativeTlsConfig<T::Config>;
    type Transport = NativeTlsTransport<T::Transport>;

    fn peer_addr(transport: &Self::Transport) -> Result<SocketAddr> {
        match transport {
            Tcp(t) => T::peer_addr(t),
            Tls(t) => T::peer_addr(t.get_ref()),
        }
    }

    async fn connect(url: &Url, config: &Self::Config) -> Result<Self::Transport> {
        match url.scheme() {
            "https" => {
                let mut http = url.clone();
                http.set_scheme("http").ok();
                http.set_port(url.port_or_known_default()).ok();
                let inner_stream = T::connect(&http, config.as_ref()).await?;
                Ok(Tls(async_native_tls::connect(url, inner_stream)
                    .await
                    .map_err(|e| {
                        Error::new(ErrorKind::Other, e.to_string())
                    })?))
            }

            "http" => Ok(Tcp(T::connect(&url, config.as_ref()).await?)),

            unknown => Err(Error::new(
                ErrorKind::InvalidInput,
                format!("unknown scheme {}", unknown),
            )),
        }
    }
}
