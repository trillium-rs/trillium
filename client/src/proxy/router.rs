use super::Proxy;
use crate::Url;
use std::{fmt, sync::Arc};

/// Chooses the proxy for each request, or none to connect directly.
///
/// A router is consulted once per request, with its original URL, before connecting. A redirect
/// or other follow-up request is routed again for its own URL; retrying protocol negotiation on
/// the same request is not. A [`Proxy`] that a router returns still applies its own schemes and
/// bypass entries, so a request it doesn't carry connects directly.
///
/// Implemented for:
///
/// - [`Proxy`], which routes the requests its schemes and bypass entries allow;
/// - functions `Fn(&Url) -> Option<Proxy>`;
/// - `Option<R>`, where `None` routes nothing;
/// - arrays and `Vec`s of routers, which return the first router's proxy that isn't `None`.
///
/// ```
/// use trillium_client::{Proxy, Url};
///
/// let corporate = Proxy::new("http://proxy.corp.example:8080")?;
/// let router = move |url: &Url| {
///     url.host_str()?
///         .ends_with(".corp.example")
///         .then(|| corporate.clone())
/// };
/// # trillium_client::Client::new(trillium_testing::client_config()).with_proxy(router);
/// # Ok::<(), std::io::Error>(())
/// ```
pub trait ProxyRouter: Send + Sync + 'static {
    /// The proxy for a request to `url`, or `None` to connect directly.
    fn route(&self, url: &Url) -> Option<Proxy>;
}

impl ProxyRouter for Proxy {
    fn route(&self, url: &Url) -> Option<Proxy> {
        self.carries(url).then(|| self.clone())
    }
}

impl<F> ProxyRouter for F
where
    F: Fn(&Url) -> Option<Proxy> + Send + Sync + 'static,
{
    fn route(&self, url: &Url) -> Option<Proxy> {
        self(url).and_then(|proxy| proxy.route(url))
    }
}

impl<R: ProxyRouter> ProxyRouter for Option<R> {
    fn route(&self, url: &Url) -> Option<Proxy> {
        self.as_ref()?.route(url)
    }
}

impl<R: ProxyRouter> ProxyRouter for Vec<R> {
    fn route(&self, url: &Url) -> Option<Proxy> {
        self.iter().find_map(|router| router.route(url))
    }
}

impl<R: ProxyRouter, const N: usize> ProxyRouter for [R; N] {
    fn route(&self, url: &Url) -> Option<Proxy> {
        self.iter().find_map(|router| router.route(url))
    }
}

#[derive(Clone)]
pub(crate) struct ArcedProxyRouter(Arc<dyn ProxyRouter>);

impl ArcedProxyRouter {
    pub(crate) fn new(router: impl ProxyRouter) -> Self {
        Self(Arc::new(router))
    }

    pub(crate) fn route(&self, url: &Url) -> Option<Proxy> {
        self.0.route(url)
    }
}

impl fmt::Debug for ArcedProxyRouter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyRouter").finish_non_exhaustive()
    }
}
