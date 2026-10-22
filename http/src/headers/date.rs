//! Thread-local cache for the response `Date` header.
//!
//! Every response within a given wall-clock second carries the same `Date`
//! value, so each thread formats it at most once per second and clones the
//! cached [`HeaderValue`] for the rest. This amortizes the calendar-formatting
//! work to once per second per thread. The clone still allocates the value's
//! backing string — an HTTP date is 29 bytes, past the inline small-string
//! threshold — but the per-response cost drops from two allocations plus
//! formatting to a single allocation.
//!
//! Noticing that the second has changed would take a precise wall-clock read on
//! every response, so the cache instead records, on a coarse monotonic clock,
//! when the next second begins. A coarse read skips the precise clock's work,
//! at the cost of rolling the header over up to one coarse tick late (1–4 ms
//! on Linux).
//!
//! The cache is deliberately thread-local rather than a shared `Arc`/`ArcSwap`:
//! handing every response an owned shared value would bounce a single refcount
//! cache line across all cores, the opposite of trillium's thread-per-core
//! direction. Per-thread state keeps the hot path contention-free.

use crate::HeaderValue;
use coarsetime::Instant;
use std::{
    cell::RefCell,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct CachedDate {
    /// When the next wall-clock second begins, and with it a new `Date`.
    expires: Instant,
    value: HeaderValue,
}

thread_local! {
    static CACHE: RefCell<Option<CachedDate>> = const { RefCell::new(None) };
}

/// A [`HeaderValue`] for the current `Date`, ready to insert into a response's
/// headers. Formatting is amortized to once per second per thread.
pub(crate) fn current_date_header() -> HeaderValue {
    // `Instant::now` would also store to a process-wide atomic on every call.
    let now = Instant::now_without_cache_update();
    CACHE.with_borrow_mut(|cache| date_header(cache, now, SystemTime::now))
}

fn date_header(
    cache: &mut Option<CachedDate>,
    now: Instant,
    wall_clock: impl FnOnce() -> SystemTime,
) -> HeaderValue {
    if let Some(cached) = cache
        && now < cached.expires
    {
        return cached.value.clone();
    }

    let wall = wall_clock();
    let into_second = wall.duration_since(UNIX_EPOCH).map_or(Duration::ZERO, |d| {
        Duration::from_nanos(u64::from(d.subsec_nanos()))
    });
    let value = HeaderValue::from(httpdate::fmt_http_date(wall));
    *cache = Some(CachedDate {
        expires: now + Duration::from_secs(1).saturating_sub(into_second).into(),
        value: value.clone(),
    });
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_formed() {
        let s = current_date_header();
        let s = s.as_str().expect("date is utf8");
        assert_eq!(s.len(), 29, "IMF-fixdate is fixed-length");
        assert!(s.ends_with(" GMT"), "got {s:?}");
    }

    #[test]
    fn cached_until_the_next_second() {
        let start = Instant::now_without_cache_update();
        let wall = UNIX_EPOCH + Duration::from_millis(1_700_000_000_250);
        let mut cache = None;
        let first = date_header(&mut cache, start, || wall);

        let later = start + Duration::from_millis(740).into();
        let cached = date_header(&mut cache, later, || panic!("should not read the clock"));
        assert_eq!(cached, first);

        let next_second = start + Duration::from_millis(760).into();
        let refreshed = date_header(&mut cache, next_second, || {
            wall + Duration::from_millis(760)
        });
        assert_ne!(refreshed, first);
        assert_eq!(refreshed.as_str(), Some("Tue, 14 Nov 2023 22:13:21 GMT"));
    }
}
