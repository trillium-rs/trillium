//! Upstream selectors
use std::{borrow::Cow, fmt::Debug};
use trillium::Conn;
use url::Url;

#[cfg(feature = "upstream-connection-counting")]
mod connection_counting;
#[cfg(feature = "upstream-random")]
mod random;
mod round_robin;

#[cfg(feature = "upstream-connection-counting")]
pub use connection_counting::ConnectionCounting;
#[cfg(feature = "upstream-random")]
pub use random::RandomSelector;
pub use round_robin::RoundRobin;

/// a trait for selecting the correct upstream
pub trait UpstreamSelector: Debug + Send + Sync + 'static {
    /// does what it says on the label
    fn determine_upstream(&self, conn: &mut Conn) -> Option<Url>;

    /// turn self into a `Box<dyn UpstreamSelector>`
    fn boxed(self) -> Box<dyn UpstreamSelector>
    where
        Self: Sized,
    {
        Box::new(self.into_upstream())
    }
}

impl UpstreamSelector for Box<dyn UpstreamSelector> {
    fn determine_upstream(&self, conn: &mut Conn) -> Option<Url> {
        UpstreamSelector::determine_upstream(&**self, conn)
    }

    fn boxed(self) -> Box<dyn UpstreamSelector> {
        self
    }
}

/// represents something that can be used as an upstream selector
///
/// This primarily exists so &str can be used as a synonym for `Url`.
/// All `UpstreamSelector`s also are `IntoUpstreamSelector`
pub trait IntoUpstreamSelector {
    /// the type that Self will be transformed into
    type UpstreamSelector: UpstreamSelector;
    /// transform self into the upstream selector
    fn into_upstream(self) -> Self::UpstreamSelector;
}

impl<U: UpstreamSelector> IntoUpstreamSelector for U {
    type UpstreamSelector = Self;

    fn into_upstream(self) -> Self {
        self
    }
}

impl IntoUpstreamSelector for &str {
    type UpstreamSelector = Url;

    fn into_upstream(self) -> Url {
        let url = match Url::try_from(self) {
            Ok(url) => url,
            Err(_) => panic!("could not convert proxy target into a url"),
        };

        assert!(!url.cannot_be_a_base(), "{url} cannot be a base");
        url
    }
}

impl IntoUpstreamSelector for String {
    type UpstreamSelector = Url;

    fn into_upstream(self) -> Url {
        (&*self).into_upstream()
    }
}

#[derive(Debug, Copy, Clone)]
/// an upstream selector for forward proxy
pub struct ForwardProxy;
impl UpstreamSelector for ForwardProxy {
    fn determine_upstream(&self, conn: &mut Conn) -> Option<Url> {
        conn.path_and_query().parse().ok()
    }
}

impl UpstreamSelector for Url {
    fn determine_upstream(&self, conn: &mut Conn) -> Option<Url> {
        join_within_base(self, conn.path_and_query())
    }
}

/// Resolve `path_and_query` against `base`, refusing to proxy anything that
/// escapes the base's directory.
///
/// `Url::join` resolves `..` per RFC 3986 §5.2.4, which lets a request walk out
/// of a base that carries a path prefix — base `…/app/` with a request path of
/// `/../secret` resolves to `/secret`. Following the `starts_with` containment
/// check that `trillium-static` uses for filesystem roots, an escape yields
/// `None` so the base prefix stays a real boundary. `%2e%2e` is recognized as a
/// `..` segment during parsing and normalized like a literal one, so encoded
/// traversal is caught by the same check; `%2f` is left encoded and rides
/// through as opaque path content.
fn join_within_base(base: &Url, path_and_query: &str) -> Option<Url> {
    // Without a trailing slash, `Url::join`'s relative-reference resolution
    // replaces the base's last path segment instead of appending under it (base
    // `…/api` + `/foo` → `…/foo`), silently dropping the mount prefix.
    let base = if base.path().ends_with('/') {
        Cow::Borrowed(base)
    } else {
        let mut owned = base.clone();
        owned.set_path(&format!("{}/", base.path()));
        Cow::Owned(owned)
    };

    // Prefix with `./` so `Url::join` treats the request path as a relative
    // *path* rather than a relative *reference*. Without this, a colon in the
    // first path segment (e.g. `/trillium::Handler`) is parsed as a URL scheme,
    // yielding a `cannot_be_a_base` url. See RFC 3986 §4.2.
    let upstream = base
        .join(&format!("./{}", path_and_query.trim_start_matches('/')))
        .ok()?;

    let base_path = base.path();
    let base_dir = &base_path[..=base_path.rfind('/').unwrap_or(0)];
    upstream.path().starts_with(base_dir).then_some(upstream)
}

impl<F> UpstreamSelector for F
where
    F: Fn(&mut Conn) -> Option<Url> + Debug + Send + Sync + 'static,
{
    fn determine_upstream(&self, conn: &mut Conn) -> Option<Url> {
        self(conn)
    }
}

#[cfg(test)]
mod tests {
    use super::join_within_base;
    use url::Url;

    fn base(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn dot_segments_cannot_escape_a_base_path_prefix() {
        let base = base("http://upstream.example/app/");

        // A `..` that would climb out of the `/app/` mount is refused entirely,
        // rather than silently proxying a path the operator never exposed. The
        // `url` crate decodes `%2e` before resolving, so percent-encoded
        // traversal is caught by the same containment check.
        for escape in [
            "/../secret",
            "/../../etc/passwd",
            "/a/../../secret",
            "/%2e%2e/secret",
        ] {
            assert_eq!(
                join_within_base(&base, escape),
                None,
                "{escape} escaped the base prefix"
            );
        }

        // Normalization *within* the mount is fine and stays contained.
        assert_eq!(join_within_base(&base, "/foo").unwrap().path(), "/app/foo");
        assert_eq!(join_within_base(&base, "/a/../b").unwrap().path(), "/app/b");
    }

    #[test]
    fn resolution_never_leaves_the_base_host() {
        let base = base("http://upstream.example/");
        for input in [
            "/https://sneaky.com",
            "/../secret",
            "/a/../b",
            "/trillium::Handler",
            "//sneaky.com/path",
        ] {
            if let Some(url) = join_within_base(&base, input) {
                assert_eq!(
                    url.host_str(),
                    Some("upstream.example"),
                    "{input} left the base host -> {url}"
                );
            }
        }
    }

    #[test]
    fn base_without_trailing_slash_keeps_its_prefix() {
        // A base mounted without a trailing slash appends under its final
        // segment rather than having it replaced by relative-reference
        // resolution.
        let base = base("http://upstream.example/api");
        assert_eq!(
            join_within_base(&base, "/users").unwrap().as_str(),
            "http://upstream.example/api/users"
        );
        // Traversal still can't climb out of the normalized prefix.
        assert_eq!(join_within_base(&base, "/../secret"), None);
    }

    #[test]
    fn query_and_first_segment_colons_survive_containment() {
        let base = base("http://upstream.example/");
        let url = join_within_base(&base, "/trillium::Handler?x=1&y=2").unwrap();
        assert_eq!(url.path(), "/trillium::Handler");
        assert_eq!(url.query(), Some("x=1&y=2"));
    }
}
