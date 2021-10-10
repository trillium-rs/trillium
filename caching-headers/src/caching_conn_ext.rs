use crate::CacheControlHeader;
use etag::EntityTag;
use std::{str::FromStr, time::SystemTime};
use trillium::{HeaderName, KnownHeaderName};

/// Provides an extension trait for both [`trillium::Headers`] and
/// also [`trillium::Conn`] for setting and getting various parsed
/// caching headers.
pub trait CachingHeadersExt: Sized {
    /// returns an [`EntityTag`] if these headers contain an `Etag` header.
    fn etag(&self) -> Option<EntityTag>;
    /// sets an etag header from an EntityTag.
    fn set_etag(&mut self, entity_tag: &EntityTag);

    /// returns a parsed timestamp if these headers contain a `Last-Modified` header.
    fn last_modified(&self) -> Option<SystemTime>;
    /// sets a formatted `Last-Modified` header from a timestamp.
    fn set_last_modified(&mut self, system_time: SystemTime);

    /// returns a parsed [`CacheControlHeader`] if these headers
    /// include a `Cache-Control` header. Note that if this is called
    /// on a [`Conn`], it returns the request [`Cache-Control`]
    /// header.
    fn cache_control(&self) -> Option<CacheControlHeader>;
    /// sets a `Cache-Control` header on these headers. Note that this
    /// is valid in both request and response contexts, and specific
    /// directives have different meanings.
    fn set_cache_control(&mut self, cache_control: impl Into<CacheControlHeader>);

    /// returns a parsed `If-Modified-Since` header if one exists
    fn if_modified_since(&self) -> Option<SystemTime>;
    /// returns a parsed [`EntityTag`] header if there is an `If-None-Match` header.
    fn if_none_match(&self) -> Option<EntityTag>;

    /// sets the Vary header to a collection of Into<HeaderName>
    fn set_vary<I, N>(&mut self, vary: I)
    where
        I: IntoIterator<Item = N>,
        N: Into<HeaderName<'static>>;

    /// chainable method to set cache control and return self. primarily useful on Conn
    fn with_cache_control(mut self, cache_control: impl Into<CacheControlHeader>) -> Self {
        self.set_cache_control(cache_control);
        self
    }

    /// chainable method to set last modified and return self. primarily useful on Conn
    fn with_last_modified(mut self, system_time: SystemTime) -> Self {
        self.set_last_modified(system_time);
        self
    }

    /// chainable method to set etag and return self. primarily useful on Conn
    fn with_etag(mut self, entity_tag: &EntityTag) -> Self {
        self.set_etag(entity_tag);
        self
    }

    /// chainable method to set vary and return self. primarily useful on Conn
    fn with_vary<I, N>(mut self, vary: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<HeaderName<'static>>,
    {
        self.set_vary(vary);
        self
    }
}

impl CachingHeadersExt for trillium::Conn {
    fn etag(&self) -> Option<EntityTag> {
        self.inner().response_headers().etag()
    }

    fn set_etag(&mut self, entity_tag: &EntityTag) {
        self.inner_mut().response_headers_mut().set_etag(entity_tag)
    }

    fn last_modified(&self) -> Option<SystemTime> {
        self.inner().response_headers().last_modified()
    }

    fn set_last_modified(&mut self, system_time: SystemTime) {
        self.inner_mut()
            .response_headers_mut()
            .set_last_modified(system_time)
    }

    fn cache_control(&self) -> Option<CacheControlHeader> {
        self.inner().request_headers().cache_control()
    }

    fn set_cache_control(&mut self, cache_control: impl Into<CacheControlHeader>) {
        self.inner_mut()
            .response_headers_mut()
            .set_cache_control(cache_control)
    }

    fn if_modified_since(&self) -> Option<SystemTime> {
        self.inner().request_headers().if_modified_since()
    }

    fn if_none_match(&self) -> Option<EntityTag> {
        self.inner().request_headers().if_none_match()
    }

    fn set_vary<I, N>(&mut self, vary: I)
    where
        I: IntoIterator<Item = N>,
        N: Into<HeaderName<'static>>,
    {
        self.inner_mut().response_headers_mut().set_vary(vary)
    }
}

impl CachingHeadersExt for trillium::Headers {
    fn etag(&self) -> Option<EntityTag> {
        self.get_str(KnownHeaderName::Etag)
            .and_then(|etag| etag.parse().ok())
    }

    fn set_etag(&mut self, entity_tag: &EntityTag) {
        let string = entity_tag.to_string();
        self.insert(KnownHeaderName::Etag, string);
    }

    fn last_modified(&self) -> Option<SystemTime> {
        self.get_str(KnownHeaderName::LastModified)
            .and_then(|x| httpdate::parse_http_date(x).ok())
    }

    fn set_last_modified(&mut self, system_time: SystemTime) {
        self.insert(
            KnownHeaderName::LastModified,
            httpdate::fmt_http_date(system_time),
        );
    }

    fn cache_control(&self) -> Option<CacheControlHeader> {
        self.get_str(KnownHeaderName::CacheControl)
            .and_then(|cc| cc.parse().ok())
    }

    fn set_cache_control(&mut self, cache_control: impl Into<CacheControlHeader>) {
        self.insert(
            KnownHeaderName::CacheControl,
            cache_control.into().to_string(),
        );
    }

    fn if_modified_since(&self) -> Option<SystemTime> {
        self.get_str(KnownHeaderName::IfModifiedSince)
            .and_then(|h| httpdate::parse_http_date(h).ok())
    }

    fn if_none_match(&self) -> Option<EntityTag> {
        self.get_str(KnownHeaderName::IfNoneMatch)
            .and_then(|etag| EntityTag::from_str(etag).ok())
    }

    fn set_vary<I, N>(&mut self, vary: I)
    where
        I: IntoIterator<Item = N>,
        N: Into<HeaderName<'static>>,
    {
        self.insert(
            KnownHeaderName::Vary,
            vary.into_iter()
                .map(|n| n.into().to_string())
                .collect::<Vec<_>>()
                .join(","),
        );
    }
}
