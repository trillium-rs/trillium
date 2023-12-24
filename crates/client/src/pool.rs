use crossbeam_queue::ArrayQueue;
use dashmap::{mapref::entry::Entry, DashMap};
use std::{
    borrow::Borrow,
    cmp::Eq,
    fmt::{self, Debug, Formatter},
    hash::Hash,
    sync::Arc,
    time::Instant,
};

pub const DEFAULT_CONNECTIONS: usize = 16;

pub struct PoolEntry<V> {
    item: V,
    expiry: Option<Instant>,
}

impl<V: Debug> Debug for PoolEntry<V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("PoolEntry")
            .field("item", &self.item)
            .field("expiry", &self.expiry)
            .finish()
    }
}

impl<V> PoolEntry<V> {
    pub fn new(item: V, expiry: Option<Instant>) -> Self {
        Self { item, expiry }
    }

    pub fn is_expired(&self) -> bool {
        match self.expiry {
            None => false,
            Some(instant) => instant < Instant::now(),
        }
    }

    pub fn take(self) -> Option<V> {
        if self.is_expired() {
            None
        } else {
            Some(self.item)
        }
    }
}

pub struct PoolSet<V>(Arc<ArrayQueue<PoolEntry<V>>>);
impl<V: Debug> Debug for PoolSet<V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PoolSet").field(&self.0).finish()
    }
}

impl<V> Default for PoolSet<V> {
    fn default() -> Self {
        Self::new(DEFAULT_CONNECTIONS)
    }
}

impl<V> Clone for PoolSet<V> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<V> PoolSet<V> {
    pub fn insert(&self, entry: PoolEntry<V>) {
        self.0.force_push(entry);
    }

    pub fn new(size: usize) -> Self {
        Self(Arc::new(ArrayQueue::new(size)))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<V> Iterator for PoolSet<V> {
    type Item = PoolEntry<V>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.pop()
    }
}

pub struct Pool<K, V> {
    pub(crate) max_set_size: usize,
    connections: Arc<DashMap<K, PoolSet<V>>>,
}

struct Connections<'a, K, V>(&'a DashMap<K, PoolSet<V>>);
impl<K, V> Debug for Connections<'_, K, V>
where
    K: Hash + Debug + Eq,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut map = f.debug_map();
        for item in self.0 {
            let (k, v) = item.pair();
            map.entry(&k, &v.0.len());
        }

        map.finish()
    }
}

impl<K, V> Debug for Pool<K, V>
where
    K: Hash + Debug + Eq,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pool")
            .field("max_set_size", &self.max_set_size)
            .field("connections", &Connections(&self.connections))
            .finish()
    }
}

impl<K, V> Clone for Pool<K, V> {
    fn clone(&self) -> Self {
        Self {
            connections: Arc::clone(&self.connections),
            max_set_size: self.max_set_size,
        }
    }
}

impl<K, V> Default for Pool<K, V>
where
    K: Hash + Debug + Eq,
{
    fn default() -> Self {
        Self {
            connections: Default::default(),
            max_set_size: DEFAULT_CONNECTIONS,
        }
    }
}

impl<K, V> Pool<K, V>
where
    K: Hash + Debug + Eq + Clone + Debug,
{
    #[allow(dead_code)]
    pub fn new(max_set_size: usize) -> Self {
        Self {
            connections: Default::default(),
            max_set_size,
        }
    }

    pub fn insert(&self, k: K, entry: PoolEntry<V>) {
        log::debug!("saving connection to {:?}", &k);
        match self.connections.entry(k) {
            Entry::Occupied(o) => {
                o.get().insert(entry);
            }

            Entry::Vacant(v) => {
                let pool_set = PoolSet::new(self.max_set_size);
                pool_set.insert(entry);
                v.insert(pool_set);
            }
        }
    }

    #[allow(dead_code)]
    pub fn keys(&self) -> impl Iterator<Item = K> + '_ {
        self.connections.iter().map(|k| (*k.key()).clone())
    }

    pub fn candidates<Q>(&self, key: &Q) -> impl Iterator<Item = V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.connections
            .get(key)
            .map(|poolset| poolset.clone().filter_map(|v| v.take()))
            .into_iter()
            .flatten()
    }

    pub fn cleanup(&self) {
        self.connections.retain(|_k, v| !v.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use trillium_server_common::Url;

    use super::*;

    #[test]
    fn basic_pool_functionality() {
        let pool = Pool::default();
        for n in 0..5 {
            pool.insert(String::from("127.0.0.1:8080"), PoolEntry::new(n, None));
        }

        assert_eq!(pool.candidates("127.0.0.1:8080").next(), Some(0));
        assert_eq!(
            pool.candidates("127.0.0.1:8080").collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn eviction() {
        let pool = Pool::new(5);
        for n in 0..10 {
            pool.insert(
                Url::parse("http://127.0.0.1:8080").unwrap().origin(),
                PoolEntry::new(n, None),
            );
        }

        assert_eq!(
            pool.candidates(&Url::parse("http://127.0.0.1:8080").unwrap().origin())
                .collect::<Vec<_>>(),
            vec![5, 6, 7, 8, 9]
        );
    }

    #[test]
    fn cleanup() {
        let pool = Pool::new(5);
        for n in 0..10 {
            pool.insert(
                Url::parse("http://127.0.0.1:8080").unwrap().origin(),
                PoolEntry::new(n, None),
            );
            pool.insert(
                Url::parse("http://0.0.0.0:1234").unwrap().origin(),
                PoolEntry::new(n * 100, None),
            );
        }
        assert_eq!(pool.keys().count(), 2);
        pool.cleanup(); // no change
        assert_eq!(pool.keys().count(), 2);
        let _ = pool
            .candidates(&Url::parse("http://0.0.0.0:1234").unwrap().origin())
            .collect::<Vec<_>>();
        assert_eq!(pool.keys().count(), 2); // haven't cleaned up
        pool.cleanup();
        assert_eq!(pool.keys().count(), 1);
    }
}
