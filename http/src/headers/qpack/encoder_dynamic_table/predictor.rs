//! Mnemonic predictor for QPACK encoder indexing decisions.
//!
//! Cheap approximate "have I seen this name / this `(name, value)` pair recently?" check,
//! consulted by the encoder planner before deciding whether to insert a new dynamic-table
//! entry. If the predictor says no (first sighting), the planner emits a literal; if yes
//! (likely repeat), the planner invests in an insert.
//!
//! Faithful to ls-qpack's "mnemonic technique": two parallel ring buffers of recent
//! `name_hash` and `nameval_hash` values, sharing one cursor. Every header writes both
//! dimensions into the same slot. [`seen`](MnemonicPredictor::seen) reports matches in
//! each dimension independently; [`remember`](MnemonicPredictor::remember) writes the new
//! slot.
//!
//! The two dimensions drive different indexing strategies:
//! - `nameval_seen` — "I've seen this exact pair before" — drives insert-then-reference (cache the
//!   whole entry).
//! - `name_seen` — "I've seen this name before with some value" — drives the name-only insert
//!   branch (cache just the name so future lines can use a dynamic name reference). Plumbed today
//!   but not yet consumed; phase 4 adds the strategy.
//!
//! ## Shared cursor
//!
//! The two arrays share one cursor because every header adds both a `name_hash` and a
//! `nameval_hash` — the observations are paired. ls-qpack takes the same approach with its
//! `struct lsqpack_hist_el { unsigned he_hashes[N_HES]; }`; independent tuning would need
//! evidence the dimensions saturate at different depths, which neither ls-qpack nor our
//! corpus has shown.
//!
//! ## Read-then-write ordering
//!
//! The planner computes hashes once per header via [`MnemonicPredictor::hash`], calls
//! [`seen`](MnemonicPredictor::seen) for the decision, and defers
//! [`remember`](MnemonicPredictor::remember) until after planning completes. Matches
//! ls-qpack's invariant that a header is *only* "seen" when a prior header hashed the
//! same way — a header is never visible to its own `seen()` query.
//!
//! ## Hash
//!
//! Uses std [`DefaultHasher`] (SipHash-1-3) truncated to 32 bits, with a `0xff` separator
//! between name and value in `nameval_hash` so `"ab"+"c"` and `"a"+"bc"` don't collide.
//! Determinism caveat: `DefaultHasher`'s algorithm is documented as "not specified across
//! releases", so corpus-test byte outputs may drift slightly when the compiler is
//! upgraded. The corpus tests assert correctness (roundtrip equivalence), not exact byte
//! counts, so this is informational only. Phase 6's EMAs will further decouple the
//! predictor from byte-stable outputs anyway.
//!
//! ## False-positive floor
//!
//! Both arrays start zero-initialized. A header whose `name_hash` or `nameval_hash`
//! happens to be `0` would appear "seen" during warm-up. Probability ~1/2³² per header
//! per dimension — negligible, not worth the cost of a fill counter.

use std::hash::{DefaultHasher, Hasher};

/// Ring buffer size for the mnemonic predictor. ls-qpack defaults to ~28 entries; we
/// match that here. Phase 6 will replace this constant with an EMA-driven auto-tuner.
const MNEMONIC_HISTORY_SIZE: usize = 28;

/// Precomputed `(name_hash, nameval_hash)` pair for one header. The planner computes this
/// once per header and threads the same value through both [`MnemonicPredictor::seen`]
/// (read) and [`MnemonicPredictor::remember`] (write) so hashing happens once.
#[derive(Clone, Copy, Debug)]
pub(super) struct HashPair {
    pub(super) name: u32,
    pub(super) nameval: u32,
}

/// Per-dimension result of a [`MnemonicPredictor::seen`] query.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Seen {
    /// True if any slot matches the query's `name_hash`. Drives the name-only insert
    /// branch — "I've seen this name with some value; cache just the name." Plumbed
    /// today; consumed starting in phase 4.
    #[allow(dead_code)] // consumed by phase-4 name-only insert branch
    pub(super) name: bool,
    /// True if any slot matches the query's `nameval_hash`. Drives insert-then-reference.
    pub(super) nameval: bool,
}

/// Two parallel fixed-size circular buffers of recent `name_hash` and `nameval_hash`
/// values, sharing one cursor. One lives on [`TableState`] per connection; all access is
/// already serialized by the `TableState` mutex.
///
/// [`TableState`]: super::state::TableState
#[derive(Debug)]
pub(super) struct MnemonicPredictor {
    name_hashes: [u32; MNEMONIC_HISTORY_SIZE],
    nameval_hashes: [u32; MNEMONIC_HISTORY_SIZE],
    cursor: usize,
}

impl MnemonicPredictor {
    pub(super) const fn new() -> Self {
        Self {
            name_hashes: [0; MNEMONIC_HISTORY_SIZE],
            nameval_hashes: [0; MNEMONIC_HISTORY_SIZE],
            cursor: 0,
        }
    }

    /// Compute the `(name_hash, nameval_hash)` pair for a header. Pure associated
    /// function — no ring access. Callers reuse the same [`HashPair`] across
    /// [`seen`](Self::seen) and [`remember`](Self::remember) within one header's encode.
    pub(super) fn hash(name: &[u8], value: &[u8]) -> HashPair {
        HashPair {
            name: name_hash(name),
            nameval: nameval_hash(name, value),
        }
    }

    /// Report whether either dimension's hash appears in the ring. Pure read — the ring
    /// is unchanged, so callers can query multiple times between `remember` calls.
    pub(super) fn seen(&self, h: HashPair) -> Seen {
        Seen {
            name: self.name_hashes.contains(&h.name),
            nameval: self.nameval_hashes.contains(&h.nameval),
        }
    }

    /// Write `(h.name, h.nameval)` at the cursor and advance. Intended to be called after
    /// the planner's decision for the header completes, so the header isn't visible to
    /// its own earlier [`seen`](Self::seen) query.
    pub(super) fn remember(&mut self, h: HashPair) {
        self.name_hashes[self.cursor] = h.name;
        self.nameval_hashes[self.cursor] = h.nameval;
        self.cursor = (self.cursor + 1) % MNEMONIC_HISTORY_SIZE;
    }
}

fn name_hash(name: &[u8]) -> u32 {
    let mut h = DefaultHasher::new();
    h.write(name);
    #[allow(clippy::cast_possible_truncation)]
    let truncated = h.finish() as u32;
    truncated
}

fn nameval_hash(name: &[u8], value: &[u8]) -> u32 {
    let mut h = DefaultHasher::new();
    h.write(name);
    // Length-agnostic separator so `"ab" + "c"` and `"a" + "bc"` don't collide.
    h.write_u8(0xff);
    h.write(value);
    // Intentional truncation — 32 bits is adequate for a 28-entry ring and keeps the
    // ring's cache footprint small. Collision probability at N=28 is ~4 × 10⁻⁸ per probe.
    #[allow(clippy::cast_possible_truncation)]
    let truncated = h.finish() as u32;
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Combined check+write helper used to port the phase-3 tests that exercised the old
    /// combined `index()` API. Production callers go through the split
    /// [`seen`](MnemonicPredictor::seen) / [`remember`](MnemonicPredictor::remember) API
    /// via the planner; this helper keeps tests terse.
    fn check_nameval(p: &mut MnemonicPredictor, name: &[u8], value: &[u8]) -> bool {
        let h = MnemonicPredictor::hash(name, value);
        let seen = p.seen(h);
        p.remember(h);
        seen.nameval
    }

    fn check_name(p: &mut MnemonicPredictor, name: &[u8], value: &[u8]) -> bool {
        let h = MnemonicPredictor::hash(name, value);
        let seen = p.seen(h);
        p.remember(h);
        seen.name
    }

    #[test]
    fn first_sighting_is_unseen_second_is_seen() {
        let mut p = MnemonicPredictor::new();
        assert!(!check_nameval(&mut p, b"x-a", b"1"));
        assert!(check_nameval(&mut p, b"x-a", b"1"));
    }

    #[test]
    fn different_values_under_same_name_are_independent_in_nameval() {
        let mut p = MnemonicPredictor::new();
        assert!(!check_nameval(&mut p, b"x-a", b"1"));
        assert!(!check_nameval(&mut p, b"x-a", b"2"));
        assert!(check_nameval(&mut p, b"x-a", b"1"));
        assert!(check_nameval(&mut p, b"x-a", b"2"));
    }

    #[test]
    fn same_nameval_pair_matches_itself() {
        let mut p = MnemonicPredictor::new();
        assert!(!check_nameval(&mut p, b"content-type", b"application/json"));
        assert!(check_nameval(&mut p, b"content-type", b"application/json"));
        assert!(!check_nameval(&mut p, b"content-type", b"application/xml"));
    }

    #[test]
    fn ring_evicts_target_after_full_cycle_of_distinct_entries() {
        let mut p = MnemonicPredictor::new();
        assert!(!check_nameval(&mut p, b"x-target", b"v"));
        for i in 0..MNEMONIC_HISTORY_SIZE {
            let name = format!("pad-{i}");
            assert!(!check_nameval(&mut p, name.as_bytes(), b"v"));
        }
        assert!(!check_nameval(&mut p, b"x-target", b"v"));
    }

    #[test]
    fn nameval_separator_prevents_concat_collision() {
        let mut p = MnemonicPredictor::new();
        assert!(!check_nameval(&mut p, b"x-ab", b"c"));
        assert!(!check_nameval(&mut p, b"x-a", b"bc"));
    }

    #[test]
    fn name_dimension_sees_repeat_with_different_value() {
        // Same name, different value → name-dimension hits on 2nd; nameval does not.
        let mut p = MnemonicPredictor::new();
        let h1 = MnemonicPredictor::hash(b"x-a", b"1");
        p.remember(h1);
        let h2 = MnemonicPredictor::hash(b"x-a", b"2");
        let seen = p.seen(h2);
        assert!(
            seen.name,
            "name_hash should match prior entry under same name"
        );
        assert!(
            !seen.nameval,
            "nameval_hash should not match when value differs",
        );
    }

    #[test]
    fn distinct_names_are_independent_in_name_dimension() {
        let mut p = MnemonicPredictor::new();
        assert!(!check_name(&mut p, b"x-a", b"1"));
        assert!(!check_name(&mut p, b"x-b", b"1"));
        assert!(check_name(&mut p, b"x-a", b"2"));
        assert!(check_name(&mut p, b"x-b", b"2"));
    }

    #[test]
    fn seen_does_not_mutate_the_ring() {
        // Multiple seen() calls between writes must be idempotent — the ring only
        // changes when remember() runs.
        let mut p = MnemonicPredictor::new();
        let h1 = MnemonicPredictor::hash(b"x-a", b"1");
        p.remember(h1);
        let h2 = MnemonicPredictor::hash(b"x-b", b"1");
        let _ = p.seen(h2);
        let _ = p.seen(h2);
        let _ = p.seen(h2);
        assert!(p.seen(h1).nameval);
        assert!(!p.seen(h2).nameval);
    }
}
