use std::fmt::{self, Debug, Formatter};
use trillium::{Conn, KnownHeaderName};
use url::{Origin, Url};

/// Which pages are allowed to open a websocket to this handler.
#[derive(Default)]
pub(crate) enum OriginPolicy {
    /// The `Origin` must name the same host as the request's `Host`/`:authority`.
    #[default]
    SameOrigin,

    /// Any origin, including none.
    Any,

    /// The `Origin` must be one of these.
    List(Vec<Origin>),

    Predicate(OriginPredicate),
}

impl Debug for OriginPolicy {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::SameOrigin => f.write_str("SameOrigin"),
            Self::Any => f.write_str("Any"),
            Self::List(origins) => f.debug_tuple("List").field(origins).finish(),
            Self::Predicate(_) => f
                .debug_tuple("Predicate")
                .field(&format_args!(".."))
                .finish(),
        }
    }
}

type PredicateFn = Box<dyn Fn(Option<&str>) -> bool + Send + Sync + 'static>; //for clippy

pub(crate) struct OriginPredicate(PredicateFn);

impl<F> From<F> for OriginPredicate
where
    F: Fn(Option<&str>) -> bool + Send + Sync + 'static,
{
    fn from(f: F) -> Self {
        Self(Box::new(f))
    }
}

impl OriginPolicy {
    pub(crate) fn list<'a>(origins: impl IntoIterator<Item = &'a str>) -> Self {
        Self::List(origins.into_iter().map(parse_allowed_origin).collect())
    }

    pub(crate) fn allows(&self, conn: &Conn) -> bool {
        let origin = conn.request_headers().get_str(KnownHeaderName::Origin);

        match self {
            // The `Origin` header is a browser-supplied statement about which page opened the
            // socket. Non-browser clients omit it entirely, and there is nothing to check.
            Self::Any => true,
            Self::Predicate(OriginPredicate(predicate)) => predicate(origin),
            Self::List(allowed) => origin.is_none_or(|origin| {
                Url::parse(origin).is_ok_and(|url| allowed.contains(&url.origin()))
            }),
            Self::SameOrigin => origin.is_none_or(|origin| same_origin(origin, conn.host())),
        }
    }
}

/// Scheme is deliberately not compared: a tls-terminating proxy leaves this server seeing an
/// `Origin` of `https://` for a request whose `Host` it received over plain http. Ports are
/// compared only when both sides name one, for the same reason — proxies routinely map an
/// external port onto a different local one.
fn same_origin(origin: &str, authority: Option<&str>) -> bool {
    let (Some(authority), Ok(origin)) = (authority, Url::parse(origin)) else {
        return false;
    };

    // `Host` and `:authority` are not urls; borrowing a scheme lets `url` do the host parsing,
    // including ipv6 brackets and idn.
    let Ok(authority) = Url::parse(&format!("http://{authority}")) else {
        return false;
    };

    let (Some(origin_host), Some(authority_host)) = (origin.host(), authority.host()) else {
        return false;
    };

    origin_host == authority_host
        && match (origin.port(), authority.port()) {
            (Some(origin_port), Some(authority_port)) => origin_port == authority_port,
            _ => true,
        }
}

/// # Panics
///
/// Panics if the provided string is not a url with a scheme and a host, or if it carries anything
/// beyond scheme, host, and port. `Origin` never contains a path, so an allowed origin written as
/// `https://example.com/app` would silently match every page on `example.com`.
fn parse_allowed_origin(origin: &str) -> Origin {
    let url = Url::parse(origin)
        .unwrap_or_else(|error| panic!("could not parse allowed origin `{origin}`: {error}"));

    assert!(
        url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
            && url.username().is_empty()
            && url.password().is_none(),
        "allowed origin `{origin}` must contain only a scheme, host, and optional port"
    );

    let origin_tuple = url.origin();

    assert!(
        origin_tuple.is_tuple(),
        "allowed origin `{origin}` does not have a host"
    );

    origin_tuple
}
