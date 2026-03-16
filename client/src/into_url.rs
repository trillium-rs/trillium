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
            (Err(ParseError::RelativeUrlWithoutBase), Some(base)) => base
                .join(self.trim_start_matches('/'))
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
