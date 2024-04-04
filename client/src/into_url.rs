use crate::{Error, Result};
use std::str::FromStr;
use trillium_server_common::url::{ParseError, Url};

/// attempt to construct a url, with base if present
pub trait IntoUrl {
    /// attempt to construct a url, with base if present
    fn into_url(self, base: Option<&Url>) -> Result<Url>;
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
}

impl IntoUrl for String {
    #[inline(always)]
    fn into_url(self, base: Option<&Url>) -> Result<Url> {
        self.as_str().into_url(base)
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
