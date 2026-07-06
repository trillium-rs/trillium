use async_lock::OnceCell;
use crossbeam_queue::ArrayQueue;
use dashmap::{DashMap, mapref::entry::Entry};
use std::{
    borrow::Borrow,
    fmt::{self, Debug, Formatter},
    hash::Hash,
    sync::{Arc, Weak},
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

/// Shared handle to a single in-flight connection establishment for one pool key.
///
/// The primer (the caller that won the race to connect) fills this via a [`PrimerGuard`];
/// concurrent callers await it. The inner `Option` is the outcome: `Some(v)` means a shareable
/// (multiplexed) connection is now pooled and `v` is a clone of it to use directly; `None` means
/// no shareable connection was produced — a non-multiplexed result, or a failed/cancelled connect
/// — and the awaiting caller should establish its own.
type Pending<V> = Arc<OnceCell<Option<V>>>;

/// The reusable connections for one pool key: a queue of ready (resolved) entries plus, at most,
/// one in-flight connect placeholder.
///
/// `ready` carries the existing behavior — h1 keepalive transports and live h2 connections, taken
/// by drain ([`Pool::candidates`]) or shared by clone ([`Pool::peek_candidate_classify`]).
/// `pending` is the single-in-flight coordination slot, only ever read or written while holding the
/// [`DashMap`] entry lock, so it needs no lock of its own.
pub struct PoolSet<V> {
    ready: Arc<ArrayQueue<PoolEntry<V>>>,
    pending: Option<Pending<V>>,
}

impl<V: Debug> Debug for PoolSet<V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("PoolSet")
            .field("ready", &self.ready)
            .field("pending", &self.pending.is_some())
            .finish()
    }
}

impl<V> Default for PoolSet<V> {
    fn default() -> Self {
        Self::new(DEFAULT_CONNECTIONS)
    }
}

impl<V> Clone for PoolSet<V> {
    fn clone(&self) -> Self {
        Self {
            ready: Arc::clone(&self.ready),
            pending: self.pending.clone(),
        }
    }
}

impl<V> PoolSet<V> {
    pub fn insert(&self, entry: PoolEntry<V>) {
        self.ready.force_push(entry);
    }

    pub fn new(size: usize) -> Self {
        Self {
            ready: Arc::new(ArrayQueue::new(size)),
            pending: None,
        }
    }

    /// Whether this set holds neither a ready connection nor an in-flight connect. An in-flight
    /// connect keeps the key alive so [`Pool::reap`] can't drop the placeholder out from under
    /// the primer.
    pub fn is_empty(&self) -> bool {
        self.ready.is_empty() && self.pending.is_none()
    }

    /// Pop the ready queue, re-pushing only entries that haven't expired. Bounded by the queue's
    /// snapshot length so a concurrent insert can't make it loop forever.
    pub fn drain_expired(&self) {
        let mut remaining = self.ready.len();
        while remaining > 0
            && let Some(entry) = self.ready.pop()
        {
            remaining -= 1;
            if !entry.is_expired() {
                self.ready.force_push(entry);
            }
        }
    }
}

impl<V> Iterator for PoolSet<V> {
    type Item = PoolEntry<V>;

    fn next(&mut self) -> Option<Self::Item> {
        self.ready.pop()
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
            map.entry(&k, &v.ready.len());
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
    K: Hash + Debug + Eq + Clone,
{
    #[allow(dead_code)]
    pub fn new(max_set_size: usize) -> Self {
        Self {
            connections: Default::default(),
            max_set_size,
        }
    }

    pub fn insert(&self, k: K, entry: PoolEntry<V>) {
        log::debug!("saving connection to {:?}", k);
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

    pub fn candidates<Q>(&self, key: &Q) -> impl Iterator<Item = V> + use<Q, K, V>
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

    /// Drop expired entries and resolved connect placeholders from every set, then drop any set
    /// left with neither a ready connection nor an in-flight connect. Unlike a plain empty-set
    /// sweep, this reclaims a set that is non-empty only because it holds expired entries an idle
    /// origin never came back to pop — the case a background reaper exists to catch.
    pub fn reap(&self) {
        self.connections.retain(|_k, set| {
            set.drain_expired();
            // A resolved placeholder has finished its coordination work — waiters hold their own
            // Arc to the cell — but it retains a clone of the pooled value, so an expired
            // connection would otherwise survive the sweep through it. An unresolved cell is a
            // live in-flight connect and keeps the key alive.
            if matches!(&set.pending, Some(cell) if cell.get().is_some()) {
                set.pending = None;
            }
            !set.is_empty()
        });
    }

    /// Returns a clone of a live candidate for `key` without removing it from the pool, asking
    /// `classify` how to treat each non-expired entry: hand out, keep but skip, or drop. Returns
    /// the first entry classified as [`Available`][PoolEntryStatus::Available]; entries classified
    /// as [`Busy`][PoolEntryStatus::Busy] are re-pushed and skipped; entries classified as
    /// [`Dead`][PoolEntryStatus::Dead] are dropped from the pool.
    ///
    /// Useful for multiplexed connections whose health and availability are independent — e.g.
    /// an HTTP/2 connection might be alive (`Available` or `Busy`) but at its
    /// `MAX_CONCURRENT_STREAMS` cap (`Busy`); a separate dead-connection signal evicts via
    /// `Dead`.
    pub fn peek_candidate_classify<Q, F>(&self, key: &Q, mut classify: F) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
        V: Clone,
        F: FnMut(&V) -> PoolEntryStatus,
    {
        let pool_set = self.connections.get(key)?;
        // Bound the loop by the queue's snapshot length to guarantee termination even if every
        // entry classifies as `Busy` (re-push would otherwise feed `pop` indefinitely).
        let mut remaining = pool_set.ready.len();
        while remaining > 0
            && let Some(entry) = pool_set.ready.pop()
        {
            remaining -= 1;
            if entry.is_expired() {
                continue;
            }
            match classify(&entry.item) {
                PoolEntryStatus::Available => {
                    let value = entry.item.clone();
                    pool_set.ready.force_push(entry);
                    return Some(value);
                }
                PoolEntryStatus::Busy => {
                    pool_set.ready.force_push(entry);
                }
                PoolEntryStatus::Dead => {}
            }
        }
        None
    }

    /// Acquire a multiplexed connection for `key`, coalescing concurrent cold-start connects.
    ///
    /// Under the per-key entry lock (held only for this synchronous call — never across the
    /// connect itself), this returns one of:
    ///
    /// - [`Acquire::Ready`] — an existing connection classified
    ///   [`Available`][PoolEntryStatus::Available]; use it directly.
    /// - [`Acquire::Await`] — another caller is already establishing a connection; await the
    ///   returned handle, then use its `Some` value or, on `None`, connect for yourself.
    /// - [`Acquire::Primer`] — no live or in-flight connection: you are the primer. Connect, then
    ///   resolve the returned [`PrimerGuard`].
    ///
    /// `classify` treats existing ready entries exactly as [`peek_candidate_classify`] does. A
    /// previously-resolved placeholder is reaped here (the caller becomes a new primer), so a
    /// connection that has since died or been taken doesn't wedge the key.
    ///
    /// [`peek_candidate_classify`]: Self::peek_candidate_classify
    pub fn acquire(&self, key: K, mut classify: impl FnMut(&V) -> PoolEntryStatus) -> Acquire<K, V>
    where
        V: Clone,
    {
        let mut slot = self.connections.entry(key.clone()).or_default();
        let pool_set = slot.value_mut();

        let mut remaining = pool_set.ready.len();
        while remaining > 0
            && let Some(entry) = pool_set.ready.pop()
        {
            remaining -= 1;
            if entry.is_expired() {
                continue;
            }
            match classify(&entry.item) {
                PoolEntryStatus::Available => {
                    let value = entry.item.clone();
                    pool_set.ready.force_push(entry);
                    return Acquire::Ready(value);
                }
                PoolEntryStatus::Busy => {
                    pool_set.ready.force_push(entry);
                }
                PoolEntryStatus::Dead => {}
            }
        }

        // A pending cell that has *not* yet been initialized is an in-flight connect to await. One
        // that has been initialized is stale (the primer finished and any resulting connection has
        // since left `ready`); fall through and become a fresh primer, reaping it.
        if matches!(&pool_set.pending, Some(cell) if cell.get().is_none()) {
            return Acquire::Await(pool_set.pending.clone().unwrap());
        }

        let pending: Pending<V> = Arc::new(OnceCell::new());
        pool_set.pending = Some(pending.clone());
        Acquire::Primer(PrimerGuard {
            pool: self.clone(),
            key,
            pending,
            resolved: false,
        })
    }
}

impl<K, V> Pool<K, V> {
    /// A non-owning handle to this pool's connection map. A background reaper holds one of these so
    /// that it self-terminates once every owning [`Client`][crate::Client] handle has dropped,
    /// rather than keeping the pool (and its connections) alive forever.
    pub fn downgrade(&self) -> WeakPool<K, V> {
        WeakPool {
            connections: Arc::downgrade(&self.connections),
            max_set_size: self.max_set_size,
        }
    }
}

/// A non-owning handle to a [`Pool`], obtained via [`Pool::downgrade`]. [`upgrade`][Self::upgrade]
/// returns a live `Pool` only while at least one owning handle survives.
pub struct WeakPool<K, V> {
    max_set_size: usize,
    connections: Weak<DashMap<K, PoolSet<V>>>,
}

impl<K, V> Debug for WeakPool<K, V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("WeakPool")
            .field("max_set_size", &self.max_set_size)
            .field("live", &(self.connections.strong_count() > 0))
            .finish()
    }
}

impl<K, V> Clone for WeakPool<K, V> {
    fn clone(&self) -> Self {
        Self {
            max_set_size: self.max_set_size,
            connections: self.connections.clone(),
        }
    }
}

impl<K, V> WeakPool<K, V> {
    /// Recover an owning [`Pool`] if any owning handle is still alive, otherwise `None`.
    pub fn upgrade(&self) -> Option<Pool<K, V>> {
        Some(Pool {
            connections: self.connections.upgrade()?,
            max_set_size: self.max_set_size,
        })
    }
}

/// Outcome of [`Pool::acquire`].
pub enum Acquire<K, V> {
    /// An existing connection classified [`Available`][PoolEntryStatus::Available]; use it.
    Ready(V),
    /// Another caller is establishing a connection; await this handle. It resolves to `Some` (a
    /// clone of the now-pooled shared connection) or `None` (no shareable connection — connect for
    /// yourself).
    Await(Pending<V>),
    /// No live or in-flight connection: you are the primer. Connect, then resolve the guard.
    Primer(PrimerGuard<K, V>),
}

/// Held by the primer (the caller [`Pool::acquire`] elected to establish the connection) for the
/// duration of its connect. Resolving it publishes the outcome to any waiters; dropping it without
/// resolving (a connect error or cancellation) wakes them to connect for themselves.
pub struct PrimerGuard<K, V> {
    pool: Pool<K, V>,
    key: K,
    pending: Pending<V>,
    resolved: bool,
}

impl<K, V> PrimerGuard<K, V>
where
    K: Hash + Debug + Eq + Clone,
    V: Clone,
{
    /// The primer produced a shareable (multiplexed) connection: pool it as a ready entry and wake
    /// waiters with a clone.
    pub fn resolve_ready(mut self, value: V, expiry: Option<Instant>) {
        self.pool
            .insert(self.key.clone(), PoolEntry::new(value.clone(), expiry));
        let _ = self.pending.set_blocking(Some(value));
        self.resolved = true;
    }

    /// The primer produced no shareable connection (a non-multiplexed result it will use itself):
    /// wake waiters to connect on their own.
    pub fn resolve_absent(mut self) {
        let _ = self.pending.set_blocking(None);
        self.resolved = true;
    }
}

impl<K, V> Drop for PrimerGuard<K, V> {
    fn drop(&mut self) {
        // A primer dropped without resolving (connect error or cancellation) must still wake
        // waiters — to `None`, so they connect for themselves rather than hang. `set_blocking`
        // won't block: nothing else initializes this cell.
        if !self.resolved {
            let _ = self.pending.set_blocking(None);
        }
    }
}

/// Classification of a pool entry's current usability, returned by the closure passed to
/// [`Pool::peek_candidate_classify`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PoolEntryStatus {
    /// Hand this entry out for the caller's request.
    Available,
    /// Keep this entry in the pool but skip it for this request — e.g. an HTTP/2 connection
    /// at its `MAX_CONCURRENT_STREAMS` cap that will become available again as streams close.
    Busy,
    /// Drop this entry from the pool — e.g. an HTTP/2 connection that received GOAWAY or whose
    /// liveness probe failed.
    Dead,
}

#[cfg(test)]
mod tests {
    use super::*;
    use trillium_server_common::Url;

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
    fn reap_drops_emptied_sets() {
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
        pool.reap(); // no change: both sets hold live entries
        assert_eq!(pool.keys().count(), 2);
        let _ = pool
            .candidates(&Url::parse("http://0.0.0.0:1234").unwrap().origin())
            .collect::<Vec<_>>();
        assert_eq!(pool.keys().count(), 2); // drained by candidates() but set not yet reaped
        pool.reap();
        assert_eq!(pool.keys().count(), 1);
    }

    #[test]
    fn reap_drains_expired_within_nonempty_sets() {
        use std::time::Duration;
        let pool = Pool::new(5);
        let key = Url::parse("http://127.0.0.1:8080").unwrap().origin();
        let past = Instant::now() - Duration::from_secs(1);
        for n in 0..3 {
            pool.insert(key.clone(), PoolEntry::new(n, Some(past)));
        }
        // The set is non-empty — it holds three expired entries an idle origin never popped — so
        // an empty-set-only sweep would retain it and leak those fds. reap() drains the expired
        // entries and then drops the emptied set.
        assert_eq!(pool.keys().count(), 1);
        pool.reap();
        assert_eq!(pool.keys().count(), 0);
    }

    #[test]
    fn reap_drops_resolved_placeholders() {
        use std::time::Duration;
        let pool = Pool::new(5);
        let key = Url::parse("http://127.0.0.1:8080").unwrap().origin();
        let Acquire::Primer(guard) = pool.acquire(key.clone(), |_: &u8| unreachable!()) else {
            panic!("empty pool should elect a primer");
        };
        guard.resolve_ready(1, Some(Instant::now() - Duration::from_secs(1)));
        assert_eq!(pool.keys().count(), 1);
        // The expired ready entry drains, and the resolved placeholder — whose held clone would
        // otherwise keep both the key and the connection alive — goes with it.
        pool.reap();
        assert_eq!(pool.keys().count(), 0);
    }

    #[test]
    fn downgrade_upgrade_tracks_owning_handle() {
        let pool = Pool::<String, u8>::new(5);
        let weak = pool.downgrade();
        assert!(weak.upgrade().is_some());
        drop(pool);
        assert!(weak.upgrade().is_none());
    }
}
