use crate::{Error, Method, Result};
use std::{
    borrow::Cow,
    net::{IpAddr, SocketAddr},
    str::FromStr,
};
use trillium_server_common::url::{ParseError, Url};

/// attempt to construct a url, with base if present
pub trait IntoUrl {
    /// attempt to construct a url, with base if present
    fn into_url(self, base: Option<&Url>) -> Result<Url>;

    /// Returns an explicit request target override for the given method, if this input represents
    /// a special-form target like `*` (for OPTIONS) or `host:port` (for CONNECT).
    fn request_target(&self, _method: Method) -> Option<Cow<'static, str>> {
        None
    }
}

impl IntoUrl for Url {
    fn into_url(self, base: Option<&Url>) -> Result<Url> {
        if self.cannot_be_a_base() {
            return Err(Error::UnexpectedUriFormat);
        }

        if base.is_some_and(|base| !self.as_str().starts_with(base.as_str())) {
            Err(Error::UnexpectedUriFormat)
        } else {
            Ok(self)
        }
    }
}

impl IntoUrl for &str {
    fn into_url(self, base: Option<&Url>) -> Result<Url> {
        match (Url::from_str(self), base) {
            (Ok(url), base) => url.into_url(base),
            // Prefix with `./` so `Url::join` treats the path as a relative
            // *path* rather than a relative *reference*. Without this, a colon in
            // the first path segment (e.g. `/trillium::Handler`) is parsed as a
            // URL scheme, yielding a `cannot_be_a_base` url. See RFC 3986 §4.2.
            (Err(ParseError::RelativeUrlWithoutBase), Some(base)) => base
                .join(&format!("./{}", self.trim_start_matches('/')))
                .map_err(|_| Error::UnexpectedUriFormat),
            _ => Err(Error::UnexpectedUriFormat),
        }
    }

    fn request_target(&self, method: Method) -> Option<Cow<'static, str>> {
        match method {
            Method::Connect if !self.contains('/') => Url::from_str(&format!("http://{self}"))
                .ok()
                .map(|_| Cow::Owned(self.to_string())),

            Method::Options if *self == "*" => Some(Cow::Borrowed("*")),
            _ => None,
        }
    }
}

impl IntoUrl for String {
    #[inline(always)]
    fn into_url(self, base: Option<&Url>) -> Result<Url> {
        self.as_str().into_url(base)
    }

    fn request_target(&self, method: Method) -> Option<Cow<'static, str>> {
        self.as_str().request_target(method)
    }
}

impl<S: AsRef<str>> IntoUrl for &[S] {
    fn into_url(self, base: Option<&Url>) -> Result<Url> {
        let Some(mut url) = base.cloned() else {
            return Err(Error::UnexpectedUriFormat);
        };
        url.path_segments_mut()
            .map_err(|_| Error::UnexpectedUriFormat)?
            .pop_if_empty()
            .extend(self);
        Ok(url)
    }
}

impl<S: AsRef<str>, const N: usize> IntoUrl for [S; N] {
    fn into_url(self, base: Option<&Url>) -> Result<Url> {
        self.as_slice().into_url(base)
    }
}

impl<S: AsRef<str>> IntoUrl for Vec<S> {
    fn into_url(self, base: Option<&Url>) -> Result<Url> {
        self.as_slice().into_url(base)
    }
}

impl IntoUrl for SocketAddr {
    fn into_url(self, base: Option<&Url>) -> Result<Url> {
        let scheme = if self.port() == 443 { "https" } else { "http" };
        format!("{scheme}://{self}").into_url(base)
    }

    fn request_target(&self, method: Method) -> Option<Cow<'static, str>> {
        (method == Method::Connect).then(|| self.to_string().into())
    }
}

impl IntoUrl for IpAddr {
    /// note that http is assumed regardless of port
    fn into_url(self, base: Option<&Url>) -> Result<Url> {
        match self {
            IpAddr::V4(v4) => format!("http://{v4}"),
            IpAddr::V6(v6) => format!("http://[{v6}]"),
        }
        .into_url(base)
    }
}

#[cfg(test)]
mod tests {
    use super::IntoUrl;
    use trillium_server_common::url::Url;

    fn base() -> Url {
        Url::parse("https://something.real/").unwrap()
    }

    /// The security-critical invariant: a relative path resolved against a base
    /// must never point at a host other than the base's. A colon in the first
    /// path segment (`/x:y`) or an embedded absolute url (`/https://sneaky.com`)
    /// used to escape the base origin, redirecting the request to an
    /// attacker-controlled host. Rejecting an input (`Err`) is safe; resolving to
    /// a foreign host is the vulnerability.
    #[test]
    fn relative_paths_never_escape_the_base_origin() {
        let base = base();
        let adversarial = [
            "/https://sneaky.com",
            "/https://sneaky.com/path",
            "//sneaky.com",
            "//sneaky.com/path",
            "/\\/sneaky.com",
            "/\\\\sneaky.com",
            "/..//sneaky.com",
            "/trillium::Handler",
            "/path:with:colons",
            "/a/b?x=https://sneaky.com",
            "/@sneaky.com",
            "/user:pass@sneaky.com",
            "/foo/../../../etc",
            "/./..//sneaky.com",
            "/%2e%2e/sneaky.com",
            "/normal/path?q=1",
            "/",
        ];

        for input in adversarial {
            if let Ok(url) = input.into_url(Some(&base)) {
                assert_eq!(
                    url.host_str(),
                    Some("something.real"),
                    "input {input:?} escaped the base origin -> {url}"
                );
                assert_eq!(url.scheme(), "https", "input {input:?} -> {url}");
                assert_eq!(
                    url.port_or_known_default(),
                    base.port_or_known_default(),
                    "input {input:?} -> {url}"
                );
            }
        }
    }

    #[test]
    fn colon_in_first_segment_is_preserved_as_a_path() {
        assert_eq!(
            "/trillium::Handler"
                .into_url(Some(&base()))
                .unwrap()
                .as_str(),
            "https://something.real/trillium::Handler"
        );
    }

    #[test]
    fn embedded_absolute_url_becomes_a_path_segment() {
        assert_eq!(
            "/https://sneaky.com"
                .into_url(Some(&base()))
                .unwrap()
                .as_str(),
            "https://something.real/https://sneaky.com"
        );
    }

    #[test]
    fn same_origin_absolute_str_is_allowed() {
        assert_eq!(
            "https://something.real/allowed"
                .into_url(Some(&base()))
                .unwrap()
                .as_str(),
            "https://something.real/allowed"
        );
    }

    #[test]
    fn cross_origin_absolute_str_is_rejected() {
        assert!("https://sneaky.com/".into_url(Some(&base())).is_err());
    }
}
