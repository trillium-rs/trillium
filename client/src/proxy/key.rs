use crate::{Headers, Url};
use std::hash::{Hash, Hasher};
use trillium_server_common::url::Origin;

/// The proxy half of a connection-pool key: which proxied connections a request may reuse.
///
/// A proxied connection is shaped by the headers the proxy saw when it was opened: the `CONNECT`
/// request's headers for a tunnel, or credentials a forwarding proxy may have bound to the
/// connection. A pooled connection can carry a request only if that request would have opened it
/// the same way, so two proxies share connections exactly when they name the same proxy origin
/// with equal headers. The endpoint URL's path, and which origins a proxy carries, don't affect
/// the connection and aren't part of the key.
#[derive(PartialEq, Eq)]
pub(crate) struct ProxyKey {
    origin: Origin,
    connect_headers: Headers,
}

// `Headers` isn't `Hash`. Equal keys have equal origins, so hashing the origin alone agrees with
// `Eq`; keys that differ only in their headers share a hash and are told apart by `Eq`.
impl Hash for ProxyKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.origin.hash(state);
    }
}

impl ProxyKey {
    pub(crate) fn new(endpoint: &Url, connect_headers: Headers) -> Self {
        Self {
            origin: endpoint.origin(),
            connect_headers,
        }
    }

    pub(crate) const fn origin(&self) -> &Origin {
        &self.origin
    }

    pub(crate) const fn connect_headers(&self) -> &Headers {
        &self.connect_headers
    }
}
