use super::{NoProxy, Proxy, ProxySchemes, invalid};
use std::{fmt, io};

impl Proxy {
    /// The proxies configured by a snapshot of the process environment, as a list to install
    /// with [`Client::with_proxy`](crate::Client::with_proxy).
    ///
    /// Reads `http_proxy`/`HTTP_PROXY`, `https_proxy`/`HTTPS_PROXY`, `all_proxy`/`ALL_PROXY`, and
    /// `no_proxy`/`NO_PROXY`. Lowercase variables take precedence, including when empty. A missing
    /// or empty per-scheme proxy uses the all-proxy fallback. Uppercase `HTTP_PROXY` is ignored
    /// when `REQUEST_METHOD` is set: CGI, which always sets `REQUEST_METHOD`, passes a request's
    /// `Proxy` header to the program as `HTTP_PROXY`.
    ///
    /// The result is what the equivalent code would build: a [`Proxy::new`] for each configured
    /// endpoint, narrowed with [`Proxy::with_schemes`] to the scheme it serves (or left carrying
    /// both when only the all-proxy fallback is set), with the `no_proxy` list added to each
    /// through [`Proxy::except`]. An environment that configures no proxy produces an empty list,
    /// which connects directly, and so does a `no_proxy` entry of `*`, without reading the proxy
    /// variables it overrides.
    ///
    /// The all-proxy fallback is only read when a scheme actually falls back to it, so an
    /// unusable `all_proxy` alongside usable per-scheme proxies is not an error. Ordinary client
    /// construction does not read these variables, and this method does not read certificate
    /// environment variables or change TLS trust.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] for non-Unicode values, malformed or unsupported
    /// proxy endpoints, or malformed bypass entries. The error names the variable the value came
    /// from, and identifies the offending entry for a bypass list.
    pub fn from_env() -> io::Result<Vec<Self>> {
        Self::from_env_with(|name| {
            std::env::var_os(name)
                .map(|value| {
                    value
                        .into_string()
                        .map_err(|_| env_error(name, "value is not Unicode"))
                })
                .transpose()
        })
    }

    pub(super) fn from_env_with(
        mut get: impl FnMut(&str) -> io::Result<Option<String>>,
    ) -> io::Result<Vec<Self>> {
        fn pair(
            get: &mut impl FnMut(&str) -> io::Result<Option<String>>,
            lower: &'static str,
            upper: &'static str,
        ) -> io::Result<(&'static str, Option<String>)> {
            match get(lower)? {
                Some(value) => Ok((lower, Some(value))),
                None => Ok((upper, get(upper)?)),
            }
        }
        fn proxy(name: &str, value: Option<String>) -> io::Result<Option<Proxy>> {
            value
                .filter(|v| !v.trim().is_empty())
                .map(|v| Proxy::new(v.trim()).map_err(|error| env_error(name, error)))
                .transpose()
        }
        let (bypass_name, bypass) = pair(&mut get, "no_proxy", "NO_PROXY")?;
        let bypass = bypass.unwrap_or_default();
        // `*` turns proxying off, overriding inherited proxy variables without unsetting them, so
        // the values it overrides are never parsed.
        if bypass.split(',').any(|entry| entry.trim() == "*") {
            return Ok(Vec::new());
        }
        let exceptions =
            NoProxy::new(&bypass).map_err(|problem| env_error(bypass_name, problem))?;

        let (http_name, http_value) = if get("REQUEST_METHOD")?.is_some() {
            ("http_proxy", get("http_proxy")?)
        } else {
            pair(&mut get, "http_proxy", "HTTP_PROXY")?
        };
        let http = proxy(http_name, http_value)?;
        let (https_name, https_value) = pair(&mut get, "https_proxy", "HTTPS_PROXY")?;
        let https = proxy(https_name, https_value)?;

        // Only a scheme without its own proxy consults the fallback, so a value that will never
        // be used is never parsed.
        let all = if http.is_none() || https.is_none() {
            let (all_name, all_value) = pair(&mut get, "all_proxy", "ALL_PROXY")?;
            proxy(all_name, all_value)?
        } else {
            None
        };

        let proxies = match (http, https) {
            (None, None) => all.into_iter().collect(),
            (http, https) => [
                http.or_else(|| all.clone())
                    .map(|proxy| proxy.with_schemes(ProxySchemes::Http)),
                https
                    .or(all)
                    .map(|proxy| proxy.with_schemes(ProxySchemes::Https)),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
        };
        Ok(proxies
            .into_iter()
            .map(|proxy| proxy.with_exceptions(&exceptions))
            .collect())
    }
}

fn env_error(variable: &str, problem: impl fmt::Display) -> io::Error {
    invalid(&format!("{variable}: {problem}"))
}
