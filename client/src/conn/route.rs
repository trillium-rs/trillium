use super::Conn;
use crate::{
    Proxy, Url,
    proxy::{ProxyKey, connect::open_tunnel},
};
use std::{fmt, sync::Arc};
use trillium_http::{KnownHeaderName, Result, Version};
use trillium_server_common::{Connector, Transport, url::Origin};

#[derive(Clone, Debug, Default)]
pub(crate) enum Route {
    #[default]
    Direct,
    Forward(Proxy),
    Tunnel(Proxy),
}

impl Route {
    pub(crate) const fn is_direct(&self) -> bool {
        matches!(self, Self::Direct)
    }

    pub(crate) const fn is_forward(&self) -> bool {
        matches!(self, Self::Forward(_))
    }

    pub(crate) const fn forward_proxy(&self) -> Option<&Proxy> {
        match self {
            Self::Forward(proxy) => Some(proxy),
            Self::Direct | Self::Tunnel(_) => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct RouteKey {
    origin: Origin,
    mode: RouteModeKey,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum RouteModeKey {
    Direct,
    Forward(Arc<ProxyKey>),
    Tunnel(Arc<ProxyKey>),
}

impl RouteKey {
    fn new(url: &Url, route: &Route) -> Self {
        let mode = match route {
            Route::Direct => RouteModeKey::Direct,
            Route::Forward(proxy) => RouteModeKey::Forward(proxy.key().clone()),
            Route::Tunnel(proxy) => RouteModeKey::Tunnel(proxy.key().clone()),
        };
        Self {
            origin: url.origin(),
            mode,
        }
    }
}

impl fmt::Debug for RouteKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let proxy = match &self.mode {
            RouteModeKey::Direct => None,
            RouteModeKey::Forward(proxy) => Some(("forward", proxy.origin())),
            RouteModeKey::Tunnel(proxy) => Some(("tunnel", proxy.origin())),
        };
        f.debug_struct("RouteKey")
            .field("origin", &self.origin)
            .field("proxy", &proxy)
            .finish_non_exhaustive()
    }
}

impl Conn {
    fn needs_raw_stream(&self) -> bool {
        self.method == trillium_http::Method::Connect
            || self.protocol.is_some()
            || self.upgrade
            || self.request_headers.get(KnownHeaderName::Upgrade).is_some()
            || self.http_version == Some(Version::Http2)
    }

    pub(super) fn select_route(&mut self) {
        self.route = match self
            .client
            .proxy()
            .and_then(|router| router.route(&self.url))
        {
            None => Route::Direct,
            Some(proxy) if self.url.scheme() == "http" && !self.needs_raw_stream() => {
                Route::Forward(proxy)
            }
            Some(proxy) => Route::Tunnel(proxy),
        };
        self.route_key = Some(RouteKey::new(&self.url, &self.route));
        if self.client.proxy().is_some() {
            self.request_headers
                .remove(KnownHeaderName::ProxyAuthorization);
        }
    }

    pub(crate) fn pool_key(&self) -> RouteKey {
        self.route_key
            .clone()
            .unwrap_or_else(|| RouteKey::new(&self.url, &self.route))
    }

    pub(super) async fn connect_route(&self) -> Result<Box<dyn Transport>> {
        let connector = self.client.connector();
        match &self.route {
            Route::Direct => Ok(connector
                .connect_to(self.origin_destination().await?)
                .await?),
            Route::Forward(proxy) => Ok(connector.connect_to(proxy.destination()?).await?),
            Route::Tunnel(proxy) => {
                let origin = self.origin_destination().await?;
                let tunnel = open_tunnel(connector, proxy, &origin, self.context.config()).await?;
                Ok(connector.connect_over(tunnel, origin).await?)
            }
        }
    }
}

#[cfg(test)]
mod tests;
