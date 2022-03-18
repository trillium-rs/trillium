use dashmap::DashMap;
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    fmt::{self, Debug, Formatter},
    net::{SocketAddr, ToSocketAddrs},
    sync::Arc,
    time::Instant,
};

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

pub struct PoolSet<V>(Arc<Mutex<VecDeque<PoolEntry<V>>>>);
impl<V: Debug> Debug for PoolSet<V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PoolSet").field(&self.0).finish()
    }
}

impl<V> Default for PoolSet<V> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(VecDeque::new())))
    }
}

impl<V> Clone for PoolSet<V> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<V> PoolSet<V> {
    pub fn insert(&self, entry: PoolEntry<V>) {
        self.0.lock().push_front(entry);
    }

    pub fn len(&self) -> usize {
        self.0.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.lock().is_empty()
    }
}

impl<V> Iterator for PoolSet<V> {
    type Item = PoolEntry<V>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.lock().pop_back()
    }
}

pub struct Pool<V>(Arc<DashMap<SocketAddr, PoolSet<V>>>);

impl<V> Debug for Pool<V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct(if self.is_empty() {
            "Pool (empty)"
        } else {
            "Pool"
        });

        for item in self.0.iter() {
            let (k, v) = item.pair();
            s.field(&k.to_string(), &v.len());
        }

        s.finish()
    }
}

impl<V> Clone for Pool<V> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<V> Default for Pool<V> {
    fn default() -> Self {
        Self(Arc::new(DashMap::new()))
    }
}

impl<V> Pool<V> {
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|v| v.value().is_empty())
    }

    pub fn insert(&self, k: SocketAddr, entry: PoolEntry<V>) {
        log::debug!("saving connection to {:?}", &k);
        match self.0.entry(k) {
            dashmap::mapref::entry::Entry::Occupied(o) => {
                o.get().insert(entry);
            }
            dashmap::mapref::entry::Entry::Vacant(v) => {
                let pool_set = PoolSet::default();
                pool_set.insert(entry);
                v.insert(pool_set);
            }
        }
    }

    fn stream_for_socket_addr(&self, addr: SocketAddr) -> Option<impl Iterator<Item = V>> {
        self.0
            .get(&addr)
            .map(|x| x.clone().filter_map(|v| v.take()))
    }

    pub fn candidates(&self, addrs: impl ToSocketAddrs) -> impl Iterator<Item = V> {
        let mut sets = vec![];

        if let Ok(addrs) = addrs.to_socket_addrs() {
            for addr in addrs {
                if let Some(stream) = self.stream_for_socket_addr(addr) {
                    sets.push(stream);
                }
            }
        }

        sets.into_iter().flatten()
    }
}
