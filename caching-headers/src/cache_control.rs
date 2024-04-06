use std::{
    fmt::{Display, Write},
    ops::{Deref, DerefMut},
    str::FromStr,
    time::Duration,
};
use trillium::{Conn, Handler, HeaderValues, KnownHeaderName};
use CacheControlDirective::*;
/**
An enum representation of the
[`Cache-Control`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control)
directives.
*/
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum CacheControlDirective {
    /// [`immutable`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#revalidation_and_reloading)
    Immutable,

    /// [`max-age`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#expiration)
    MaxAge(Duration),

    /// [`max-fresh`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control#expiration)
    MaxFresh(Duration),

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

/**
A representation of the
[`Cache-Control`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Cache-Control)
header.
*/
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

    /// returns a duration if one of the directives is `max-fresh`
    pub fn max_fresh(&self) -> Option<Duration> {
        self.iter().find_map(|d| match d {
            MaxFresh(d) => Some(*d),
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

#[derive(Debug, Clone, Copy)]
pub struct CacheControlParseError;
impl std::error::Error for CacheControlParseError {}
impl Display for CacheControlParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cache control parse error")
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
                MaxFresh(d) => write!(f, "max-fresh={}", d.as_secs()),
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

impl FromStr for CacheControlHeader {
    type Err = CacheControlParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.to_ascii_lowercase()
            .split(',')
            .map(str::trim)
            .map(|directive| match directive {
                "immutable" => Ok(Immutable),
                "must-revalidate" => Ok(MustRevalidate),
                "no-cache" => Ok(NoCache),
                "no-store" => Ok(NoStore),
                "no-transform" => Ok(NoTransform),
                "only-if-cached" => Ok(OnlyIfCached),
                "private" => Ok(Private),
                "proxy-revalidate" => Ok(ProxyRevalidate),
                "public" => Ok(Public),
                "max-stale" => Ok(MaxStale(None)),
                other => match other.split_once('=') {
                    Some((directive, number)) => {
                        let seconds = number.parse().map_err(|_| CacheControlParseError)?;
                        let seconds = Duration::from_secs(seconds);
                        match directive {
                            "max-age" => Ok(MaxAge(seconds)),
                            "max-fresh" => Ok(MaxFresh(seconds)),
                            "max-stale" => Ok(MaxStale(Some(seconds))),
                            "s-maxage" => Ok(SMaxage(seconds)),
                            "stale-if-error" => Ok(StaleIfError(seconds)),
                            "stale-while-revalidate" => Ok(StaleWhileRevalidate(seconds)),
                            _ => Ok(UnknownDirective(String::from(other))),
                        }
                    }

                    None => Ok(UnknownDirective(String::from(other))),
                },
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }
}
#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn parse() {
        assert_eq!(
            CacheControlHeader(vec![NoStore]),
            "no-store".parse().unwrap()
        );

        let long = "private,no-cache,no-store,max-age=0,must-revalidate,pre-check=0,post-check=0"
            .parse()
            .unwrap();

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
            "public, max-age=604800, immutable".parse().unwrap()
        );
    }
}
