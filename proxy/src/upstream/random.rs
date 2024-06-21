use super::{IntoUpstreamSelector, UpstreamSelector};
use std::{
    fmt::Debug,
    ops::{Deref, DerefMut},
};
use trillium::Conn;
use url::Url;

#[derive(Debug)]
/// a random upstream selector
pub struct RandomSelector<T>(Vec<T>);
impl<T> UpstreamSelector for RandomSelector<T>
where
    T: UpstreamSelector,
{
    fn determine_upstream(&self, conn: &mut Conn) -> Option<Url> {
        fastrand::choice(&self.0).and_then(|u| u.determine_upstream(conn))
    }
}

impl<T> RandomSelector<T>
where
    T: UpstreamSelector,
{
    /// construct a new round robin
    pub fn new<I, U>(urls: I) -> Self
    where
        I: IntoIterator<Item = U>,
        U: IntoUpstreamSelector<UpstreamSelector = T>,
    {
        Self(urls.into_iter().map(|u| u.into_upstream()).collect())
    }
}

impl<T> Deref for RandomSelector<T>
where
    T: UpstreamSelector,
{
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<T> DerefMut for RandomSelector<T>
where
    T: UpstreamSelector,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl<U, T> Extend<U> for RandomSelector<T>
where
    T: UpstreamSelector,
    U: IntoUpstreamSelector<UpstreamSelector = T>,
{
    fn extend<I: IntoIterator<Item = U>>(&mut self, iter: I) {
        self.0.extend(iter.into_iter().map(|i| i.into_upstream()));
    }
}

impl<U, V> FromIterator<U> for RandomSelector<V>
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
