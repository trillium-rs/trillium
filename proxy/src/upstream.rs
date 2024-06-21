//! Upstream selectors
use std::fmt::Debug;
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
        conn.inner().path_and_query().parse().ok()
    }
}

impl UpstreamSelector for Url {
    fn determine_upstream(&self, conn: &mut Conn) -> Option<Url> {
        self.join(conn.inner().path_and_query().trim_start_matches('/'))
            .ok()
    }
}

impl<F> UpstreamSelector for F
where
    F: Fn(&mut Conn) -> Option<Url> + Debug + Send + Sync + 'static,
{
    fn determine_upstream(&self, conn: &mut Conn) -> Option<Url> {
        self(conn)
    }
}
