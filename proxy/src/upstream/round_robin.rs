//! Upstream selectors
use super::{IntoUpstreamSelector, UpstreamSelector};
use std::{
    fmt::Debug,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicU8, Ordering},
};
use trillium::Conn;
use url::Url;

#[derive(Debug)]
/// an upstream selector that dispatches to upstreams in the order that they're specified, wrapping
pub struct RoundRobin<T>(Vec<T>, AtomicU8);
impl<T> UpstreamSelector for RoundRobin<T>
where
    T: UpstreamSelector,
{
    fn determine_upstream(&self, conn: &mut Conn) -> Option<Url> {
        let wrapping_count = self.1.fetch_add(1, Ordering::SeqCst);
        let index = (wrapping_count as usize) % self.0.len();
        self.0.get(index).and_then(|u| u.determine_upstream(conn))
    }
}

impl<T> RoundRobin<T>
where
    T: UpstreamSelector,
{
    /// construct a new round robin
    pub fn new<I, U>(urls: I) -> Self
    where
        I: IntoIterator<Item = U>,
        U: IntoUpstreamSelector<UpstreamSelector = T>,
    {
        Self(
            urls.into_iter().map(|u| u.into_upstream()).collect(),
            0.into(),
        )
    }
}

impl<T> Deref for RoundRobin<T>
where
    T: UpstreamSelector,
{
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<T> DerefMut for RoundRobin<T>
where
    T: UpstreamSelector,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl<U, T> Extend<U> for RoundRobin<T>
where
    T: UpstreamSelector,
    U: IntoUpstreamSelector<UpstreamSelector = T>,
{
    fn extend<I: IntoIterator<Item = U>>(&mut self, iter: I) {
        self.0.extend(iter.into_iter().map(|i| i.into_upstream()));
    }
}

impl<U, V> FromIterator<U> for RoundRobin<V>
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
