use crossbeam_queue::ArrayQueue;
use dashmap::{mapref::entry::Entry, DashMap};
use std::{
    fmt::{self, Debug, Formatter},
    net::{SocketAddr, ToSocketAddrs},
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

pub struct Pool<V> {
    pub(crate) max_set_size: usize,
    connections: Arc<DashMap<SocketAddr, PoolSet<V>>>,
}

struct Connections<'a, V>(&'a DashMap<SocketAddr, PoolSet<V>>);
impl<V> Debug for Connections<'_, V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut map = f.debug_map();
        for item in self.0 {
            let (k, v) = item.pair();
            map.entry(&k.to_string(), &v.0.len());
        }

        map.finish()
    }
}

impl<V> Debug for Pool<V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pool")
            .field("max_set_size", &self.max_set_size)
            .field("connections", &Connections(&self.connections))
            .finish()
    }
}

impl<V> Clone for Pool<V> {
    fn clone(&self) -> Self {
        Self {
            connections: Arc::clone(&self.connections),
            max_set_size: self.max_set_size,
        }
    }
}

impl<V> Default for Pool<V> {
    fn default() -> Self {
        Self {
            connections: Default::default(),
            max_set_size: DEFAULT_CONNECTIONS,
        }
    }
}

impl<V> Pool<V> {
    #[allow(dead_code)]
    pub fn new(max_set_size: usize) -> Self {
        Self {
            connections: Default::default(),
            max_set_size,
        }
    }

    pub fn insert(&self, k: SocketAddr, entry: PoolEntry<V>) {
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
    pub fn keys(&self) -> impl Iterator<Item = SocketAddr> + '_ {
        self.connections.iter().map(|k| *k.key())
    }

    fn stream_for_socket_addr(&self, addr: SocketAddr) -> Option<impl Iterator<Item = V>> {
        self.connections
            .get(&addr)
            .map(|poolset| poolset.clone().filter_map(|v| v.take()))
    }

    pub fn cleanup(&self) {
        self.connections.retain(|_k, v| !v.is_empty())
    }

    pub fn candidates<'a, 'b: 'a>(
        &'a self,
        addrs: impl ToSocketAddrs + 'b,
    ) -> impl Iterator<Item = V> + 'a {
        addrs
            .to_socket_addrs()
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|addr| self.stream_for_socket_addr(addr))
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_pool_functionality() {
        let pool = Pool::default();
        for n in 0..5 {
            pool.insert("127.0.0.1:8080".parse().unwrap(), PoolEntry::new(n, None));
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
            pool.insert("127.0.0.1:8080".parse().unwrap(), PoolEntry::new(n, None));
        }

        assert_eq!(
            pool.candidates("127.0.0.1:8080").collect::<Vec<_>>(),
            vec![5, 6, 7, 8, 9]
        );
    }

    #[test]
    fn candidates_for_multiple_to_socket_addrs() {
        let pool = Pool::new(5);
        for n in 0..10 {
            pool.insert("127.0.0.1:8080".parse().unwrap(), PoolEntry::new(n, None));
            pool.insert("[::1]:8080".parse().unwrap(), PoolEntry::new(n * 10, None));
            pool.insert(
                "0.0.0.0:1234".parse().unwrap(),
                PoolEntry::new(n * 100, None),
            );
        }

        let localhost_8080 = [
            "[::1]:8080".parse().unwrap(),
            "127.0.0.1:8080".parse().unwrap(),
        ]; // so we don't depend on test environment supporting v6

        assert_eq!(
            pool.candidates(localhost_8080.as_slice())
                .collect::<Vec<_>>(),
            vec![50, 60, 70, 80, 90, 5, 6, 7, 8, 9]
        );
    }

    #[test]
    fn when_to_socket_addrs_fails() {
        let pool = Pool::new(5);
        for n in 0..10 {
            pool.insert("127.0.0.1:8080".parse().unwrap(), PoolEntry::new(n, None));
        }

        assert_eq!(
            pool.candidates("not a socket addr").collect::<Vec<_>>(),
            vec![]
        );
    }

    #[test]
    fn cleanup() {
        let pool = Pool::new(5);
        for n in 0..10 {
            pool.insert("127.0.0.1:8080".parse().unwrap(), PoolEntry::new(n, None));
            pool.insert(
                "0.0.0.0:1234".parse().unwrap(),
                PoolEntry::new(n * 100, None),
            );
        }
        assert_eq!(pool.keys().count(), 2);
        pool.cleanup(); // no change
        assert_eq!(pool.keys().count(), 2);
        let _ = pool.candidates("0.0.0.0:1234").collect::<Vec<_>>();
        assert_eq!(pool.keys().count(), 2); // haven't cleaned up
        pool.cleanup();
        assert_eq!(pool.keys().count(), 1);
    }
}
