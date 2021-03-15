use std::collections::HashMap;
use std::collections::VecDeque;
use std::fmt::{self, Debug, Formatter};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

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
        self.0.lock().unwrap().push_front(entry);
    }

    pub fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.lock().unwrap().is_empty()
    }
}

impl<V> Iterator for PoolSet<V> {
    type Item = PoolEntry<V>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.lock().unwrap().pop_back()
    }
}

pub struct Pool<V>(Arc<RwLock<HashMap<SocketAddr, PoolSet<V>>>>);

impl<V> Debug for Pool<V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct(if self.is_empty() {
            "Pool (empty)"
        } else {
            "Pool"
        });

        for (k, v) in &*self.0.read().unwrap() {
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
        Self(Arc::new(RwLock::new(HashMap::new())))
    }
}

impl<V> Pool<V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.0.read().unwrap().values().map(|v| v.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.0.read().unwrap().values().all(|v| v.is_empty())
    }

    pub fn insert(&self, k: SocketAddr, entry: PoolEntry<V>) {
        let lock = self.0.read().unwrap();
        log::debug!("saving connection to {:?}", &k);
        if lock.contains_key(&k) {
            lock.get(&k).unwrap().insert(entry)
        } else {
            let pool_set = PoolSet::default();
            pool_set.insert(entry);
            std::mem::drop(lock);
            self.0.write().unwrap().insert(k, pool_set);
        }
    }

    fn stream_for_socket_addr(&self, addr: SocketAddr) -> Option<impl Iterator<Item = V>> {
        self.0
            .read()
            .unwrap()
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
