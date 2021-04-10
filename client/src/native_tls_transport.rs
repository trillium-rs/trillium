use futures_lite::{AsyncRead, AsyncWrite};
use trillium::async_trait;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use url::Url;

use crate::ClientTransport;

pub enum NativeTls<T> {
    Tcp(T),
    Tls(async_native_tls::TlsStream<T>),
}

impl<T: ClientTransport> AsyncRead for NativeTls<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        match &mut *self {
            NativeTls::Tcp(t) => Pin::new(t).poll_read(cx, buf),
            NativeTls::Tls(t) => Pin::new(t).poll_read(cx, buf),
        }
    }
}

impl<T: ClientTransport> AsyncWrite for NativeTls<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match &mut *self {
            NativeTls::Tcp(t) => Pin::new(t).poll_write(cx, buf),
            NativeTls::Tls(t) => Pin::new(t).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            NativeTls::Tcp(t) => Pin::new(t).poll_flush(cx),
            NativeTls::Tls(t) => Pin::new(t).poll_flush(cx),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            NativeTls::Tcp(t) => Pin::new(t).poll_close(cx),
            NativeTls::Tls(t) => Pin::new(t).poll_close(cx),
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct NativeTlsConfig<Config> {
    pub tcp_config: Config,
}

impl<Config> AsRef<Config> for NativeTlsConfig<Config> {
    fn as_ref(&self) -> &Config {
        &self.tcp_config
    }
}

#[async_trait]
impl<T: ClientTransport> ClientTransport for NativeTls<T> {
    type Config = NativeTlsConfig<T::Config>;

    fn peer_addr(&self) -> std::io::Result<SocketAddr> {
        match self {
            NativeTls::Tcp(t) => t.peer_addr(),
            NativeTls::Tls(t) => t.get_ref().peer_addr(),
        }
    }

    async fn connect(url: &Url, config: &Self::Config) -> std::io::Result<Self> {
        match url.scheme() {
            "https" => {
                let mut http = url.clone();
                http.set_scheme("http").ok();
                http.set_port(url.port_or_known_default()).ok();
                let inner_stream = T::connect(&http, config.as_ref()).await?;
                Ok(Self::Tls(
                    async_native_tls::connect(url, inner_stream)
                        .await
                        .map_err(|e| {
                            dbg!(&e);
                            std::io::Error::new(ErrorKind::Other, e.to_string())
                        })?,
                ))
            }

            "http" => Ok(Self::Tcp(T::connect(&url, config.as_ref()).await?)),

            unknown => Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("unknown scheme {}", unknown),
            )),
        }
    }
}
