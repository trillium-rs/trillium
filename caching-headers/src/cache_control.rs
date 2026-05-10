use CacheControlDirective::*;
use std::{
    fmt::{Display, Write},
    ops::{Deref, DerefMut},
    time::Duration,
};
use trillium::{Conn, Handler, HeaderValues, KnownHeaderName};
/// An enum representation of the
/// [`Cache-Control`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control)
/// directives.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum CacheControlDirective {
    /// [`immutable`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#revalidation_and_reloading)
    Immutable,

    /// [`max-age`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#expiration)
    MaxAge(Duration),

    /// [`min-fresh`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#min-fresh)
    MinFresh(Duration),

    /// [`max-stale`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#expiration)
    MaxStale(Option<Duration>),

    /// [`must-revalidate`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#revalidation_and_reloading)
    MustRevalidate,

    /// [`no-cache`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#cacheability)
    NoCache,

    /// [`no-store`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#cacheability)
    NoStore,

    /// [`no-transform`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#other)
    NoTransform,

    /// [`only-if-cached`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#other)
    OnlyIfCached,

    /// [`private`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#cacheability)
    Private,

    /// [`proxy-revalidate`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#revalidation_and_reloading)
    ProxyRevalidate,

    /// [`public`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#cacheability)
    Public,

    /// [`s-maxage`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#expiration)
    SMaxage(Duration),

    /// [`stale-if-error`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#expiration)
    StaleIfError(Duration),

    /// [`stale-while-revalidate`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#expiration)
    StaleWhileRevalidate(Duration),

    /// an enum variant that will contain any unrecognized directives
    UnknownDirective(String),
}

impl Handler for CacheControlDirective {
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_response_header(KnownHeaderName::CacheControl, self.clone())
    }
}

impl Handler for CacheControlHeader {
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_response_header(KnownHeaderName::CacheControl, self.clone())
    }
}

/// A representation of the
/// [`Cache-Control`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control)
/// header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheControlHeader(Vec<CacheControlDirective>);

/// Construct a CacheControlHeader. Alias for [`CacheControlHeader::new`]
pub fn cache_control(into: impl Into<CacheControlHeader>) -> CacheControlHeader {
    into.into()
}

impl<T> From<T> for CacheControlHeader
where
    T: IntoIterator<Item = CacheControlDirective>,
{
    fn from(directives: T) -> Self {
        directives.into_iter().collect()
    }
}

impl From<CacheControlDirective> for CacheControlHeader {
    fn from(directive: CacheControlDirective) -> Self {
        Self(vec![directive])
    }
}

impl FromIterator<CacheControlDirective> for CacheControlHeader {
    fn from_iter<T: IntoIterator<Item = CacheControlDirective>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl From<CacheControlDirective> for HeaderValues {
    fn from(ccd: CacheControlDirective) -> HeaderValues {
        let header: CacheControlHeader = ccd.into();
        header.into()
    }
}

impl From<CacheControlHeader> for HeaderValues {
    fn from(cch: CacheControlHeader) -> Self {
        cch.to_string().into()
    }
}

impl CacheControlHeader {
    /// construct a new cache control header. alias for [`CacheControlHeader::from`]
    pub fn new(into: impl Into<Self>) -> Self {
        into.into()
    }

    /// returns true if one of the directives is `immutable`
    pub fn is_immutable(&self) -> bool {
        self.contains(&Immutable)
    }

    /// returns a duration if one of the directives is `max-age`
    pub fn max_age(&self) -> Option<Duration> {
        self.iter().find_map(|d| match d {
            MaxAge(d) => Some(*d),
            _ => None,
        })
    }

    /// returns a duration if one of the directives is `min-fresh`
    pub fn min_fresh(&self) -> Option<Duration> {
        self.iter().find_map(|d| match d {
            MinFresh(d) => Some(*d),
            _ => None,
        })
    }

    /// returns Some(None) if one of the directives is `max-stale` but
    /// no value is provided. returns Some(Some(duration)) if one of
    /// the directives is max-stale and includes a duration in
    /// seconds, such as `max-stale=3600`. Returns None if there is no
    /// `max-stale` directive
    pub fn max_stale(&self) -> Option<Option<Duration>> {
        self.iter().find_map(|d| match d {
            MaxStale(d) => Some(*d),
            _ => None,
        })
    }

    /// returns true if this header contains a `must-revalidate` directive
    pub fn must_revalidate(&self) -> bool {
        self.contains(&MustRevalidate)
    }

    /// returns true if this header contains a `no-cache` directive
    pub fn is_no_cache(&self) -> bool {
        self.contains(&NoCache)
    }

    /// returns true if this header contains a `no-store` directive
    pub fn is_no_store(&self) -> bool {
        self.contains(&NoStore)
    }

    /// returns true if this header contains a `no-transform`
    /// directive
    pub fn is_no_transform(&self) -> bool {
        self.contains(&NoTransform)
    }

    /// returns true if this header contains an `only-if-cached`
    /// directive
    pub fn is_only_if_cached(&self) -> bool {
        self.contains(&OnlyIfCached)
    }

    /// returns true if this header contains a `private` directive
    pub fn is_private(&self) -> bool {
        self.contains(&Private)
    }

    /// returns true if this header contains a `proxy-revalidate`
    /// directive
    pub fn is_proxy_revalidate(&self) -> bool {
        self.contains(&ProxyRevalidate)
    }

    /// returns true if this header contains a `proxy` directive
    pub fn is_public(&self) -> bool {
        self.contains(&Public)
    }

    /// returns a duration if this header contains an `s-maxage`
    /// directive
    pub fn s_maxage(&self) -> Option<Duration> {
        self.iter().find_map(|h| match h {
            SMaxage(d) => Some(*d),
            _ => None,
        })
    }

    /// returns a duration if this header contains a stale-if-error
    /// directive
    pub fn stale_if_error(&self) -> Option<Duration> {
        self.iter().find_map(|h| match h {
            StaleIfError(d) => Some(*d),
            _ => None,
        })
    }

    /// returns a duration if this header contains a
    /// stale-while-revalidate directive
    pub fn stale_while_revalidate(&self) -> Option<Duration> {
        self.iter().find_map(|h| match h {
            StaleWhileRevalidate(d) => Some(*d),
            _ => None,
        })
    }

    /// Parse a `Cache-Control` header value. Unrecognized directives are
    /// preserved as [`CacheControlDirective::UnknownDirective`] per RFC 9111
    /// §5.2; this parser is infallible.
    pub fn parse(s: &str) -> Self {
        Self(
            s.to_ascii_lowercase()
                .split(',')
                .map(str::trim)
                .filter(|directive| !directive.is_empty())
                .map(|directive| match directive {
                    "immutable" => Immutable,
                    "must-revalidate" => MustRevalidate,
                    "no-cache" => NoCache,
                    "no-store" => NoStore,
                    "no-transform" => NoTransform,
                    "only-if-cached" => OnlyIfCached,
                    "private" => Private,
                    "proxy-revalidate" => ProxyRevalidate,
                    "public" => Public,
                    "max-stale" => MaxStale(None),
                    other => match other.split_once('=') {
                        Some((directive, value)) => {
                            let seconds = value.parse().map(Duration::from_secs);
                            match (directive, seconds) {
                                ("max-age", Ok(d)) => MaxAge(d),
                                ("min-fresh", Ok(d)) => MinFresh(d),
                                ("max-stale", Ok(d)) => MaxStale(Some(d)),
                                ("s-maxage", Ok(d)) => SMaxage(d),
                                ("stale-if-error", Ok(d)) => StaleIfError(d),
                                ("stale-while-revalidate", Ok(d)) => StaleWhileRevalidate(d),
                                _ => UnknownDirective(String::from(other)),
                            }
                        }
                        None => UnknownDirective(String::from(other)),
                    },
                })
                .collect(),
        )
    }
}

impl Deref for CacheControlHeader {
    type Target = [CacheControlDirective];

    fn deref(&self) -> &Self::Target {
        self.0.as_slice()
    }
}

impl DerefMut for CacheControlHeader {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut_slice()
    }
}

impl Display for CacheControlHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut first = true;
        for directive in &self.0 {
            if first {
                first = false;
            } else {
                f.write_char(',')?;
            }

            match directive {
                Immutable => write!(f, "immutable"),
                MaxAge(d) => write!(f, "max-age={}", d.as_secs()),
                MinFresh(d) => write!(f, "min-fresh={}", d.as_secs()),
                MaxStale(Some(d)) => write!(f, "max-stale={}", d.as_secs()),
                MaxStale(None) => write!(f, "max-stale"),
                MustRevalidate => write!(f, "must-revalidate"),
                NoCache => write!(f, "no-cache"),
                NoStore => write!(f, "no-store"),
                NoTransform => write!(f, "no-transform"),
                OnlyIfCached => write!(f, "only-if-cached"),
                Private => write!(f, "private"),
                ProxyRevalidate => write!(f, "proxy-revalidate"),
                Public => write!(f, "public"),
                SMaxage(d) => write!(f, "s-maxage={}", d.as_secs()),
                StaleIfError(d) => write!(f, "stale-if-error={}", d.as_secs()),
                StaleWhileRevalidate(d) => write!(f, "stale-while-revalidate={}", d.as_secs()),
                UnknownDirective(directive) => write!(f, "{directive}"),
            }?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn parse() {
        assert_eq!(
            CacheControlHeader(vec![NoStore]),
            CacheControlHeader::parse("no-store")
        );

        let long = CacheControlHeader::parse(
            "private,no-cache,no-store,max-age=0,must-revalidate,pre-check=0,post-check=0",
        );

        assert_eq!(
            CacheControlHeader::from([
                Private,
                NoCache,
                NoStore,
                MaxAge(Duration::ZERO),
                MustRevalidate,
                UnknownDirective("pre-check=0".to_string()),
                UnknownDirective("post-check=0".to_string())
            ]),
            long
        );

        assert_eq!(
            long.to_string(),
            "private,no-cache,no-store,max-age=0,must-revalidate,pre-check=0,post-check=0"
        );

        assert_eq!(
            CacheControlHeader::from([Public, MaxAge(Duration::from_secs(604800)), Immutable]),
            CacheControlHeader::parse("public, max-age=604800, immutable")
        );
    }

    #[test]
    fn min_fresh() {
        let parsed = CacheControlHeader::parse("min-fresh=300");
        assert_eq!(parsed.min_fresh(), Some(Duration::from_secs(300)));
        assert_eq!(parsed.to_string(), "min-fresh=300");
    }

    #[test]
    fn unknown_directive_with_value_does_not_fail_header() {
        // RFC 9111 §5.2: unrecognized directives MUST be ignored, not abort
        // parsing of the rest of the header. Previously a non-numeric value on
        // an unknown directive would cause the whole header to fail to parse.
        let parsed = CacheControlHeader::parse("garbage=non-numeric, max-age=600");
        assert_eq!(parsed.max_age(), Some(Duration::from_secs(600)));
        assert!(parsed.contains(&UnknownDirective("garbage=non-numeric".into())));
    }
}
