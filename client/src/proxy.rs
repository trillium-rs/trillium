use crate::{HeaderName, HeaderValues, Headers, KnownHeaderName, Url};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{borrow::Cow, fmt, io, sync::Arc};
use trillium_server_common::Destination;

pub(crate) mod connect;
pub use connect::ProxyConnectError;

mod env;

mod key;
pub(crate) use key::ProxyKey;

mod no_proxy;
use no_proxy::NoProxy;

mod router;
pub(crate) use router::ArcedProxyRouter;
pub use router::ProxyRouter;

#[cfg(test)]
mod tests;

/// An outbound HTTP proxy: where to send requests, which origin schemes it carries, and which
/// origins bypass it.
///
/// Install with [`Client::with_proxy`](crate::Client::with_proxy), alone or as part of a
/// [`ProxyRouter`]. Plain HTTP requests are forwarded to the proxy. HTTPS requests, and HTTP
/// requests that need the origin's own connection — upgrades such as WebSocket, HTTP/2 prior
/// knowledge, and CONNECT — go through a CONNECT tunnel instead. A proxied request never falls
/// back to a direct connection.
///
/// A `Proxy-Authorization` CONNECT header, including one derived from the endpoint URL's user
/// information, is sent with both forwarded requests and tunnels; other CONNECT headers are sent
/// only with CONNECT. While a router is installed, a `Proxy-Authorization` header set on an
/// individual request is removed, including on requests sent directly. Installing a proxy does
/// not change which certificates the TLS connector trusts.
///
/// ```
/// use trillium_client::{Proxy, ProxySchemes};
///
/// let proxy = Proxy::new("http://user:pass@proxy.example:8080")?
///     .with_schemes(ProxySchemes::Https)
///     .except(["localhost", "127.0.0.0/8", "[::1]", ".internal.example"])?;
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Clone)]
pub struct Proxy(Arc<Config>);

#[derive(Clone)]
struct Config {
    endpoint: Url,
    key: Arc<ProxyKey>,
    schemes: ProxySchemes,
    exceptions: NoProxy,
}

/// Which origin URL schemes a [`Proxy`] carries. Requests for other schemes connect directly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProxySchemes {
    /// `http://` origins only.
    Http,
    /// `https://` origins only.
    Https,
    /// Both `http://` and `https://` origins. The default.
    #[default]
    Both,
}

impl ProxySchemes {
    fn includes(self, scheme: &str) -> bool {
        match self {
            Self::Http => scheme == "http",
            Self::Https => scheme == "https",
            Self::Both => matches!(scheme, "http" | "https"),
        }
    }
}

impl Proxy {
    /// A proxy at `endpoint`, carrying both HTTP and HTTPS origins.
    ///
    /// `endpoint` is an `http://` or `https://` URL; without a scheme, `http://` is assumed. URL
    /// user information becomes a Basic `Proxy-Authorization` CONNECT header, percent escapes
    /// decoded, and is removed from the stored endpoint. The path, query, and fragment don't
    /// affect connections.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] for a malformed URL, an unsupported scheme, a URL
    /// without a host, or a username containing `:`. The error never includes the credentials.
    pub fn new(endpoint: impl AsRef<str>) -> io::Result<Self> {
        let endpoint = endpoint.as_ref();
        let endpoint: Cow<'_, str> = if endpoint.contains("://") {
            endpoint.into()
        } else {
            format!("http://{endpoint}").into()
        };
        let mut endpoint = Url::parse(&endpoint).map_err(|_| invalid("invalid proxy endpoint"))?;
        let authorization = basic_authorization(&endpoint)?;
        endpoint
            .set_username("")
            .map_err(|()| invalid("invalid proxy endpoint"))?;
        endpoint
            .set_password(None)
            .map_err(|()| invalid("invalid proxy endpoint"))?;
        Destination::from_url(&endpoint)?;

        let mut connect_headers = Headers::new();
        if let Some(authorization) = authorization {
            connect_headers.insert(KnownHeaderName::ProxyAuthorization, authorization);
        }
        Ok(Self(Arc::new(Config {
            key: Arc::new(ProxyKey::new(&endpoint, connect_headers)),
            endpoint,
            schemes: ProxySchemes::Both,
            exceptions: NoProxy::default(),
        })))
    }

    /// Carry only origins with these schemes.
    #[must_use]
    pub fn with_schemes(mut self, schemes: ProxySchemes) -> Self {
        Arc::make_mut(&mut self.0).schemes = schemes;
        self
    }

    /// Send this header to the proxy with `CONNECT`, replacing any values it already has.
    ///
    /// `Proxy-Authorization` is also sent with forwarded requests. Opening a tunnel fails if the
    /// CONNECT headers include `Host`, `Content-Length`, `Transfer-Encoding`, `Connection`, or
    /// `Upgrade`: the client writes `Host` itself, and a `CONNECT` request has no body and
    /// upgrades nothing.
    #[must_use]
    pub fn with_connect_header(
        mut self,
        name: impl Into<HeaderName<'static>>,
        value: impl Into<HeaderValues>,
    ) -> Self {
        let config = Arc::make_mut(&mut self.0);
        let mut connect_headers = config.key.connect_headers().clone();
        connect_headers.insert(name, value);
        config.key = Arc::new(ProxyKey::new(&config.endpoint, connect_headers));
        self
    }

    /// Connect directly to origins matching any of these bypass entries, in addition to any
    /// already added.
    ///
    /// Each entry is one domain name, IP address, or CIDR network, written exactly. Entries are
    /// matched against the URL host without resolving DNS. A domain matches itself and its
    /// subdomains at label boundaries, so `example.com` matches `a.example.com` but not
    /// `notexample.com`; a leading `.` or `*.` is optional, and case and trailing dots are ignored.
    /// An optional `:port` limits an entry to that effective URL port; bracket an IPv6 literal that
    /// has a port, as in `[::1]:8080`. IP addresses and networks match IP-literal URLs, not
    /// addresses a domain resolves to.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`], naming the entry, for one that is malformed,
    /// empty, has surrounding whitespace, or contains a comma, and for `*`, which would leave the
    /// proxy carrying nothing.
    pub fn except<I>(mut self, entries: I) -> io::Result<Self>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        let exceptions = &mut Arc::make_mut(&mut self.0).exceptions;
        for entry in entries {
            exceptions.push(entry.as_ref())?;
        }
        Ok(self)
    }

    /// The proxy's URL, without credentials.
    #[must_use]
    pub fn endpoint(&self) -> &Url {
        &self.0.endpoint
    }

    /// The origin schemes this proxy carries.
    #[must_use]
    pub fn schemes(&self) -> ProxySchemes {
        self.0.schemes
    }

    /// The headers sent to the proxy with `CONNECT`.
    #[must_use]
    pub fn connect_headers(&self) -> &Headers {
        self.0.key.connect_headers()
    }

    /// Whether this proxy carries a request to `url`, by its schemes and bypass entries.
    fn carries(&self, url: &Url) -> bool {
        self.0.schemes.includes(url.scheme()) && !self.0.exceptions.contains(url)
    }

    fn with_exceptions(mut self, exceptions: &NoProxy) -> Self {
        Arc::make_mut(&mut self.0)
            .exceptions
            .extend(exceptions.clone());
        self
    }

    pub(crate) fn key(&self) -> &Arc<ProxyKey> {
        &self.0.key
    }

    /// The [`Destination`] for dialing the proxy itself.
    ///
    /// Talking to an HTTP proxy is a `CONNECT` exchange or a forwarded HTTP/1.1 request,
    /// so the ALPN is pinned rather than negotiated.
    pub(crate) fn destination(&self) -> io::Result<Destination> {
        let mut destination = Destination::from_url(&self.0.endpoint)?;
        destination.set_alpn([Cow::Borrowed(b"http/1.1".as_slice())]);
        Ok(destination)
    }
}

impl fmt::Debug for Proxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Proxy")
            .field("endpoint", &self.0.endpoint.as_str())
            .field("schemes", &self.0.schemes)
            .field("exceptions", &self.0.exceptions)
            .finish_non_exhaustive()
    }
}

fn basic_authorization(endpoint: &Url) -> io::Result<Option<String>> {
    if endpoint.username().is_empty() && endpoint.password().is_none() {
        return Ok(None);
    }
    let username: Vec<u8> = percent_encoding::percent_decode_str(endpoint.username()).collect();
    if username.contains(&b':') {
        return Err(invalid(
            "Basic proxy authorization username contains a colon",
        ));
    }
    let password: Vec<u8> =
        percent_encoding::percent_decode_str(endpoint.password().unwrap_or_default()).collect();
    let mut credentials = username;
    credentials.push(b':');
    credentials.extend(password);
    Ok(Some(format!("Basic {}", STANDARD.encode(credentials))))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
