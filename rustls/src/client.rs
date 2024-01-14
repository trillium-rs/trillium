use crate::RustlsTransport;
use futures_rustls::TlsConnector;
#[cfg(any(feature = "ring", feature = "aws-lc-rs"))]
use rustls::{pki_types::CertificateDer, RootCertStore};
use rustls::{pki_types::ServerName, ClientConfig};
use std::{
    fmt::{self, Debug, Formatter},
    future::Future,
    io::{Error, ErrorKind, Result},
    sync::Arc,
};
use trillium_server_common::{async_trait, Connector, Url};

#[derive(Clone, Debug)]
pub struct RustlsClientConfig(Arc<ClientConfig>);

/**
Client configuration for RustlsConnector
*/
#[derive(Clone)]
#[cfg_attr(any(feature = "ring", feature = "aws-lc-rs"), derive(Default))]
pub struct RustlsConfig<Config> {
    /// configuration for rustls itself
    pub rustls_config: RustlsClientConfig,

    /// configuration for the inner transport
    pub tcp_config: Config,
}

impl<C: Connector> RustlsConfig<C> {
    /// build a new default rustls config with this tcp config
    pub fn new(rustls_config: impl Into<RustlsClientConfig>, tcp_config: C) -> Self {
        Self {
            rustls_config: rustls_config.into(),
            tcp_config,
        }
    }
}

#[cfg(any(feature = "ring", feature = "aws-lc-rs"))]
impl Default for RustlsClientConfig {
    fn default() -> Self {
        Self(Arc::new(default_client_config()))
    }
}

#[cfg(any(feature = "ring", feature = "aws-lc-rs"))]
#[cfg(feature = "native-roots")]
fn get_rustls_native_roots() -> Option<Vec<CertificateDer<'static>>> {
    let roots = rustls_native_certs::load_native_certs();
    if let Err(ref e) = roots {
        log::warn!("rustls native certs hard error, falling back to webpki roots: {e:?}");
    }
    roots.ok()
}

#[cfg(any(feature = "ring", feature = "aws-lc-rs"))]
#[cfg(not(feature = "native-roots"))]
fn get_rustls_native_roots() -> Option<Vec<CertificateDer<'static>>> {
    None
}

#[cfg(any(feature = "ring", feature = "aws-lc-rs"))]
fn default_client_config() -> ClientConfig {
    let mut root_store = RootCertStore::empty();
    match get_rustls_native_roots() {
        Some(certs) => {
            for cert in certs {
                if let Err(e) = root_store.add(cert) {
                    log::debug!("unable to add certificate {:?}, skipping", e);
                }
            }
        }

        None => {
            root_store.extend(webpki_roots::TLS_SERVER_ROOTS.to_owned());
        }
    };

    #[cfg(all(feature = "ring", not(feature = "aws-lc-rs")))]
    let provider = rustls::crypto::ring::default_provider();
    #[cfg(feature = "aws-lc-rs")]
    let provider = rustls::crypto::aws_lc_rs::default_provider();

    ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .expect("could not enable default TLS versions")
        .with_root_certificates(root_store)
        .with_no_client_auth()
}

impl From<ClientConfig> for RustlsClientConfig {
    fn from(rustls_config: ClientConfig) -> Self {
        Self(Arc::new(rustls_config))
    }
}

impl From<Arc<ClientConfig>> for RustlsClientConfig {
    fn from(rustls_config: Arc<ClientConfig>) -> Self {
        Self(rustls_config)
    }
}

impl<C: Connector> RustlsConfig<C> {
    /// replace the tcp config
    pub fn with_tcp_config(mut self, config: C) -> Self {
        self.tcp_config = config;
        self
    }
}

impl<Config: Debug> Debug for RustlsConfig<Config> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("RustlsConfig")
            .field("rustls_config", &"..")
            .field("tcp_config", &self.tcp_config)
            .finish()
    }
}

#[async_trait]
impl<C: Connector> Connector for RustlsConfig<C> {
    type Transport = RustlsTransport<C::Transport>;

    async fn connect(&self, url: &Url) -> Result<Self::Transport> {
        match url.scheme() {
            "https" => {
                let mut http = url.clone();
                http.set_scheme("http").ok();
                http.set_port(url.port_or_known_default()).ok();

                let connector: TlsConnector = Arc::clone(&self.rustls_config.0).into();
                let domain = url
                    .domain()
                    .and_then(|dns_name| ServerName::try_from(dns_name.to_string()).ok())
                    .ok_or_else(|| Error::new(ErrorKind::Other, "missing domain"))?;

                connector
                    .connect(domain, self.tcp_config.connect(&http).await?)
                    .await
                    .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
                    .map(RustlsTransport::from)
            }

            "http" => self
                .tcp_config
                .connect(url)
                .await
                .map(RustlsTransport::from),

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
