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
//! The cache is deliberately thread-local rather than a shared `Arc`/`ArcSwap`:
//! handing every response an owned shared value would bounce a single refcount
//! cache line across all cores, the opposite of trillium's thread-per-core
//! direction. Per-thread state keeps the hot path contention-free.

use crate::HeaderValue;
use std::{
    cell::RefCell,
    time::{SystemTime, UNIX_EPOCH},
};

thread_local! {
    static CACHE: RefCell<Option<(u64, HeaderValue)>> = const { RefCell::new(None) };
}

/// A [`HeaderValue`] for the current `Date`, ready to insert into a response's
/// headers. Formatting is amortized to once per second per thread.
pub(crate) fn current_date_header() -> HeaderValue {
    let now = SystemTime::now();
    let secs = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());

    CACHE.with_borrow_mut(|cache| {
        if let Some((cached_secs, value)) = cache
            && *cached_secs == secs
        {
            return value.clone();
        }
        let value = HeaderValue::from(httpdate::fmt_http_date(now));
        *cache = Some((secs, value.clone()));
        value
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }

    #[test]
    fn well_formed_and_cached_within_a_second() {
        let before = now_secs();
        let a = current_date_header();
        let b = current_date_header();
        let after = now_secs();

        let s = a.as_str().expect("date is utf8");
        assert_eq!(s.len(), 29, "IMF-fixdate is fixed-length");
        assert!(s.ends_with(" GMT"), "got {s:?}");

        // If both reads landed in the same wall-clock second, the cache must have
        // served identical values.
        if before == after {
            assert_eq!(a, b);
        }
    }
}
