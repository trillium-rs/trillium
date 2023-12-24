//! Upstream selectors
use super::{IntoUpstreamSelector, UpstreamSelector};
use std::cmp::Ordering::*;
use std::{
    fmt::Debug,
    ops::{Deref, DerefMut},
};
use trillium::Conn;
use trillium_server_common::{CloneCounter, CloneCounterObserver};
use url::Url;

#[derive(Debug)]
/// an upstream selector that attempts to send requests to the upstream with the fewest open connections.
///
/// if there are several with the same lowest number of connections, a random upstream is chosen from them.
pub struct ConnectionCounting<T>(Vec<(T, CloneCounterObserver)>);
impl<T> ConnectionCounting<T>
where
    T: UpstreamSelector,
{
    ///
    pub fn new<I, U>(urls: I) -> Self
    where
        I: IntoIterator<Item = U>,
        U: IntoUpstreamSelector<UpstreamSelector = T>,
    {
        Self(
            urls.into_iter()
                .map(|u| (u.into_upstream(), CloneCounterObserver::new()))
                .collect(),
        )
    }
}

struct ConnectionCount(CloneCounter);

impl<T> UpstreamSelector for ConnectionCounting<T>
where
    T: UpstreamSelector,
{
    fn determine_upstream(&self, conn: &mut Conn) -> Option<Url> {
        let mut current_lowest = usize::MAX;
        let mut current_selection = vec![];
        for (u, c) in &self.0 {
            let current = c.current();
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
            conn.set_state(ConnectionCount(cc.counter()));
            u.determine_upstream(conn)
        })
    }
}

impl<T> Deref for ConnectionCounting<T>
where
    T: UpstreamSelector,
{
    type Target = [(T, CloneCounterObserver)];
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
        self.0.extend(
            iter.into_iter()
                .map(|i| (i.into_upstream(), CloneCounterObserver::new())),
        );
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
