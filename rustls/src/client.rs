use crate::RustlsTransport;
use async_rustls::TlsConnector;
use rustls::{ClientConfig, OwnedTrustAnchor, RootCertStore, ServerName};
use std::{
    fmt::{self, Debug, Formatter},
    future::Future,
    io::{Error, ErrorKind, Result},
    sync::Arc,
};
use trillium_server_common::{async_trait, Connector, Url};

/**
Client configuration for RustlsConnector
*/
#[derive(Clone)]
pub struct RustlsConfig<Config> {
    /// configuration for rustls itself
    pub rustls_config: Arc<ClientConfig>,

    /// configuration for the inner transport
    pub tcp_config: Config,
}

impl<Config: Default> Default for RustlsConfig<Config> {
    fn default() -> Self {
        let mut root_store = RootCertStore::empty();
        match rustls_native_certs::load_native_certs() {
            Ok(certs) => {
                for cert in certs {
                    if let Err(e) = root_store.add(&rustls::Certificate(cert.0)) {
                        log::debug!("unable to add certificate {:?}, skipping", e);
                    }
                }
            }

            Err(e) => {
                log::warn!(
                    "rustls native certs hard error, falling back to webpki roots: {:?}",
                    e
                );
                root_store.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(
                    |c: &async_rustls::webpki::TrustAnchor| {
                        OwnedTrustAnchor::from_subject_spki_name_constraints(
                            c.subject,
                            c.spki,
                            c.name_constraints,
                        )
                    },
                ));
            }
        };

        ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(root_store)
            .with_no_client_auth()
            .into()
    }
}

impl<Config: Default> From<ClientConfig> for RustlsConfig<Config> {
    fn from(rustls_config: ClientConfig) -> Self {
        Arc::new(rustls_config).into()
    }
}

impl<Config: Default> From<Arc<ClientConfig>> for RustlsConfig<Config> {
    fn from(rustls_config: Arc<ClientConfig>) -> Self {
        Self {
            rustls_config,
            tcp_config: Config::default(),
        }
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

                let connector: TlsConnector = Arc::clone(&self.rustls_config).into();
                let domain = url
                    .domain()
                    .and_then(|dns_name| ServerName::try_from(dns_name).ok())
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
