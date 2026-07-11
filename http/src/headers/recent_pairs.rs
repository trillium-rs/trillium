//! Per-connection ring of recently-seen `(name, value)` pair hashes.
//!
//! A small ring buffer tracking the hashes of recent header lines. The encoder consults
//! it to decide whether to invest in a dynamic-table insert: first sighting of a pair
//! returns `false`, subsequent sightings within the ring's retention window return `true`.
//! Cross-connection priming is handled separately by the observer; this ring catches
//! per-connection repetition that priming missed.
//!
//! ## Sizing
//!
//! Sized once per connection at construction from
//! [`HttpConfig::recent_pairs_size`]. Fixed for the lifetime of the connection.
//! Backed by `Box<[u32]>` so LLVM can autovectorize the [`contains`] scan in
//! [`seen`](RecentPairs::seen) without the `SmallVec` inline-vs-spilled branch.
//!
//! [`HttpConfig::recent_pairs_size`]: crate::HttpConfig::recent_pairs_size
//! [`contains`]: slice::contains
//!
//! ## Read-then-write ordering
//!
//! Callers compute the hash once via [`RecentPairs::hash`], call [`seen`](RecentPairs::seen)
//! for the indexing decision, and defer [`remember`](RecentPairs::remember) until after
//! planning the line. A header is never visible to its own earlier `seen` query.
//!
//! ## Hash
//!
//! Std [`DefaultHasher`] (SipHash-1-3) over `name + 0xff separator + value`, truncated
//! to 32 bits. Determinism caveat: `DefaultHasher`'s algorithm is documented as "not
//! specified across releases". Wire-byte outputs may drift slightly across Rust toolchains;
//! corpus tests assert correctness (roundtrip equivalence), not exact byte counts.
//!
//! ## False-positive floor
//!
//! Ring starts zero-initialized. A header whose hash happens to be `0` would appear
//! "seen" during warm-up. Probability ~1/2³² per header — negligible, not worth a fill
//! counter.

use std::hash::{DefaultHasher, Hasher};

#[derive(Debug)]
pub(in crate::headers) struct RecentPairs {
    hashes: Box<[u32]>,
    cursor: usize,
}

impl RecentPairs {
    /// Construct a ring of `size` slots. `size` is clamped to at least 1 so the ring is
    /// always usable.
    pub(in crate::headers) fn with_size(size: usize) -> Self {
        let size = size.max(1);
        Self {
            hashes: vec![0; size].into_boxed_slice(),
            cursor: 0,
        }
    }

    /// Derive a ring size from a negotiated dynamic-table capacity (bytes): roughly twice
    /// the entry count the table can hold, clamped to `[8, 256]`.
    ///
    /// The window must match table retention: a ring that remembers pairs longer than the
    /// table could have kept the corresponding entries approves inserts whose repetition
    /// already aged out — each one is a guaranteed evict/re-insert churn cycle. (An
    /// oversized ring at a 256-byte table measured *worse than not having a dynamic table
    /// at all*; matched sizing beat static-only at every capacity swept.)
    pub(in crate::headers) fn auto_size(table_capacity: usize) -> usize {
        (table_capacity / 32).clamp(8, 256)
    }

    /// Derive the warming-insert sighting threshold from a negotiated dynamic-table
    /// capacity (bytes): insert on the 2nd sighting for tiny tables, the 3rd otherwise.
    ///
    /// Real connections are frequently 1–2 responses long. An insert triggered by a
    /// second sighting pays off only from a *third* — so on a 2-response connection it is
    /// pure waste, and with a capacity-sized window that waste measured ~20% over a
    /// static-only encoder at capacities ≥ 1024. Requiring a third sighting makes short
    /// connections degrade gracefully to no-insert at a small cost on long connections.
    /// Tiny tables keep the faster trigger: their tight window (8–15 slots) only approves
    /// pairs repeating within a section or two, which is safe at any connection length.
    pub(in crate::headers) fn auto_seen_k(table_capacity: usize) -> u8 {
        if table_capacity >= 512 { 3 } else { 2 }
    }

    /// Compute the ring-indexable hash for one `(name, value)` pair. Pure associated
    /// function — no ring access. Callers reuse the same hash across [`seen`](Self::seen)
    /// and [`remember`](Self::remember) within one header's encode.
    pub(in crate::headers) fn hash(name: &[u8], value: &[u8]) -> u32 {
        let mut h = DefaultHasher::new();
        h.write(name);
        // Length-agnostic separator so `"ab" + "c"` and `"a" + "bc"` don't collide.
        h.write_u8(0xff);
        h.write(value);
        // Intentional truncation — 32 bits is adequate for ring sizes in the dozens; the
        // collision probability at typical sizes is ~10⁻⁸ per probe.
        #[allow(clippy::cast_possible_truncation)]
        let truncated = h.finish() as u32;
        truncated
    }

    /// Number of slots in the ring.
    pub(in crate::headers) fn size(&self) -> usize {
        self.hashes.len()
    }

    /// Report whether `hash` is in the ring. Pure read — the ring is unchanged, so
    /// callers can query multiple times between [`remember`](Self::remember) calls.
    pub(in crate::headers) fn seen(&self, hash: u32) -> bool {
        self.hashes.contains(&hash)
    }

    /// Count occurrences of `hash` in the ring. [`remember`](Self::remember) never
    /// dedupes, so each past sighting within the window occupies its own slot — the ring
    /// materializes a sliding-window sighting count for free. `count(h) > 0` is
    /// equivalent to [`seen`](Self::seen).
    pub(in crate::headers) fn count(&self, hash: u32) -> usize {
        self.hashes.iter().filter(|&&x| x == hash).count()
    }

    /// Write `hash` at the cursor and advance. Intended to be called after the planner's
    /// decision for the header completes, so the header isn't visible to its own earlier
    /// [`seen`](Self::seen) query.
    pub(in crate::headers) fn remember(&mut self, hash: u32) {
        self.hashes[self.cursor] = hash;
        self.cursor = (self.cursor + 1) % self.hashes.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Combined check+write helper: query `seen` for the (name, value) hash, then
    /// `remember` it, returning the seen result. Production callers go through the split
    /// API via the planner; this keeps tests terse.
    fn check(p: &mut RecentPairs, name: &[u8], value: &[u8]) -> bool {
        let h = RecentPairs::hash(name, value);
        let seen = p.seen(h);
        p.remember(h);
        seen
    }

    #[test]
    fn first_sighting_is_unseen_second_is_seen() {
        let mut p = RecentPairs::with_size(12);
        assert!(!check(&mut p, b"x-a", b"1"));
        assert!(check(&mut p, b"x-a", b"1"));
    }

    #[test]
    fn different_values_under_same_name_are_independent() {
        let mut p = RecentPairs::with_size(12);
        assert!(!check(&mut p, b"x-a", b"1"));
        assert!(!check(&mut p, b"x-a", b"2"));
        assert!(check(&mut p, b"x-a", b"1"));
        assert!(check(&mut p, b"x-a", b"2"));
    }

    #[test]
    fn nameval_separator_prevents_concat_collision() {
        let mut p = RecentPairs::with_size(12);
        assert!(!check(&mut p, b"x-ab", b"c"));
        assert!(!check(&mut p, b"x-a", b"bc"));
    }

    #[test]
    fn seen_does_not_mutate_the_ring() {
        let mut p = RecentPairs::with_size(12);
        let h1 = RecentPairs::hash(b"x-a", b"1");
        p.remember(h1);
        let h2 = RecentPairs::hash(b"x-b", b"1");
        let _ = p.seen(h2);
        let _ = p.seen(h2);
        let _ = p.seen(h2);
        assert!(p.seen(h1));
        assert!(!p.seen(h2));
    }

    #[test]
    fn ring_evicts_oldest_after_full_cycle() {
        let ring_size = 12;
        let mut p = RecentPairs::with_size(ring_size);
        assert!(!check(&mut p, b"x-target", b"v"));
        for i in 0..ring_size {
            let name = format!("pad-{i}");
            assert!(!check(&mut p, name.as_bytes(), b"v"));
        }
        // After ring_size + 1 distinct remembers, the target's slot has been overwritten.
        assert!(!check(&mut p, b"x-target", b"v"));
    }

    #[test]
    fn with_size_clamps_zero_to_one() {
        let mut p = RecentPairs::with_size(0);
        assert_eq!(p.hashes.len(), 1);
        // A 1-slot ring is functional (degenerate but doesn't panic on modulo).
        let h = RecentPairs::hash(b"x", b"y");
        p.remember(h);
        assert!(p.seen(h));
    }
}
