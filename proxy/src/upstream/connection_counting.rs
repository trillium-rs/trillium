//! Upstream selectors
use super::{IntoUpstreamSelector, UpstreamSelector};
use std::{
    cmp::Ordering::*,
    fmt::Debug,
    ops::{Deref, DerefMut},
    sync::Arc,
};
use trillium::Conn;
use url::Url;

#[derive(Debug)]
/// an upstream selector that attempts to send requests to the upstream with the fewest open
/// connections.
///
/// if there are several with the same lowest number of connections, a random upstream is chosen
/// from them.
pub struct ConnectionCounting<T>(Vec<(T, Arc<()>)>);
impl<T> ConnectionCounting<T>
where
    T: UpstreamSelector,
{
    /// Constructs a new connection counting upstream.
    pub fn new<I, U>(urls: I) -> Self
    where
        I: IntoIterator<Item = U>,
        U: IntoUpstreamSelector<UpstreamSelector = T>,
    {
        Self(
            urls.into_iter()
                .map(|u| (u.into_upstream(), Arc::new(())))
                .collect(),
        )
    }
}

#[allow(dead_code)]
struct ConnectionCount(Arc<()>);

impl<T> UpstreamSelector for ConnectionCounting<T>
where
    T: UpstreamSelector,
{
    fn determine_upstream(&self, conn: &mut Conn) -> Option<Url> {
        let mut current_lowest = usize::MAX;
        let mut current_selection = vec![];
        for (u, c) in &self.0 {
            let current = Arc::strong_count(c);
            match current.cmp(&current_lowest) {
                Less => {
                    current_lowest = current;
                    current_selection = vec![(u, c)];
                }

                Equal => {
                    current_selection.push((u, c));
                }

                Greater => {}
            }
        }

        fastrand::choice(current_selection).and_then(|(u, cc)| {
            conn.insert_state(ConnectionCount(Arc::clone(cc)));
            u.determine_upstream(conn)
        })
    }
}

impl<T> Deref for ConnectionCounting<T>
where
    T: UpstreamSelector,
{
    type Target = [(T, Arc<()>)];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<T> DerefMut for ConnectionCounting<T>
where
    T: UpstreamSelector,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl<U, T> Extend<U> for ConnectionCounting<T>
where
    T: UpstreamSelector,
    U: IntoUpstreamSelector<UpstreamSelector = T>,
{
    fn extend<I: IntoIterator<Item = U>>(&mut self, iter: I) {
        self.0
            .extend(iter.into_iter().map(|i| (i.into_upstream(), Arc::new(()))));
    }
}

impl<U, V> FromIterator<U> for ConnectionCounting<V>
where
    U: IntoUpstreamSelector<UpstreamSelector = V>,
    V: UpstreamSelector,
{
    fn from_iter<T>(urls: T) -> Self
    where
        T: IntoIterator<Item = U>,
    {
        Self::new(urls)
    }
}
