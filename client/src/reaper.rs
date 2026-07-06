use crate::{conn::H2Pooled, pool::WeakPool};
use std::{fmt::Debug, hash::Hash, time::Duration};
use trillium_server_common::{Runtime, Transport, url::Origin};

/// How often the background reaper wakes to drop expired pooled connections belonging to origins
/// that are no longer being contacted — the case that never reaps lazily, because lazy reaping
/// only happens when a request reaches for a candidate. Deliberately coarse: it only bounds
/// retention of connections that are *already* idle, where a few extra minutes cost nothing;
/// connections still in use reap the instant they're reached for.
const REAPER_INTERVAL: Duration = Duration::from_secs(300);

/// Spawn a background task per connection pool that periodically drops expired entries (and the
/// sets they empty).
///
/// Each task holds only a [`WeakPool`] handle, so it self-terminates once every owning
/// [`Client`][crate::Client] has dropped — there is no shutdown to wire up, and it never keeps a
/// pool (or its connections) alive on its own.
pub(crate) fn spawn_pool_reaper(
    runtime: Runtime,
    h1_pool: WeakPool<Origin, Box<dyn Transport>>,
    h2_pool: WeakPool<Origin, H2Pooled>,
) {
    runtime
        .clone()
        .spawn(reap_loop(runtime.clone(), REAPER_INTERVAL, h1_pool));
    runtime
        .clone()
        .spawn(reap_loop(runtime, REAPER_INTERVAL, h2_pool));
}

/// Reap `weak`'s pool every `interval` until its last owning handle drops, then return.
async fn reap_loop<K, V>(runtime: Runtime, interval: Duration, weak: WeakPool<K, V>)
where
    K: Hash + Debug + Eq + Clone,
{
    loop {
        runtime.delay(interval).await;
        match weak.upgrade() {
            Some(pool) => pool.reap(),
            None => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Pool, pool::PoolEntry};
    use std::time::Instant;
    use trillium_testing::{harness, test};

    /// An origin that stops being contacted never triggers lazy reaping (nothing pops its set),
    /// so only the background loop reclaims its expired entries. Prove that path, and that the
    /// loop lets go once the pool's last owning handle drops.
    #[test(harness)]
    async fn reaps_untouched_origin_then_exits_when_pool_drops() {
        let runtime = Runtime::new(trillium_testing::runtime());
        let pool: Pool<String, u8> = Pool::default();
        pool.insert(
            "origin".into(),
            PoolEntry::new(1, Some(Instant::now() - Duration::from_secs(1))),
        );
        assert_eq!(pool.keys().count(), 1);

        let weak = pool.downgrade();
        runtime.clone().spawn(reap_loop(
            runtime.clone(),
            Duration::from_millis(2),
            weak.clone(),
        ));

        for _ in 0..500 {
            if weak.upgrade().is_some_and(|p| p.keys().count() == 0) {
                break;
            }
            runtime.delay(Duration::from_millis(1)).await;
        }
        assert_eq!(
            pool.keys().count(),
            0,
            "reaper should drop the expired entry of an origin nothing came back to pop"
        );

        drop(pool);
        assert!(
            weak.upgrade().is_none(),
            "reaper no longer keeps the pool alive"
        );
    }
}
