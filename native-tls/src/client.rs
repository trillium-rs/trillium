use async_native_tls::TlsConnector;
use std::{
    fmt::Debug,
    future::Future,
    io::{Error, ErrorKind, Result},
    sync::Arc,
};
use trillium_server_common::{async_trait, Connector, Url};

use crate::NativeTlsTransport;

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

#[async_trait]
impl<T: Connector> Connector for NativeTlsConfig<T> {
    type Transport = NativeTlsTransport<T::Transport>;

    async fn connect(&self, url: &Url) -> Result<Self::Transport> {
        match url.scheme() {
            "https" => {
                let mut http = url.clone();
                http.set_scheme("http").ok();
                http.set_port(url.port_or_known_default()).ok();
                let inner_stream = self.tcp_config.connect(url).await?;

                self.tls_connector
                    .connect(url, inner_stream)
                    .await
                    .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
                    .map(NativeTlsTransport::from)
            }

            "http" => self
                .tcp_config
                .connect(url)
                .await
                .map(NativeTlsTransport::from),

            unknown => Err(Error::new(
                ErrorKind::InvalidInput,
                format!("unknown scheme {unknown}"),
            )),
        }
    }

    fn spawn<Fut: Future<Output = ()> + Send + 'static>(&self, fut: Fut) {
        self.tcp_config.spawn(fut)
    }
}
