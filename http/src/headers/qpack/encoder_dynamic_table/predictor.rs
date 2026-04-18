//! Mnemonic predictor for QPACK encoder indexing decisions.
//!
//! Cheap approximate "have I seen this `(name, value)` pair recently?" check, consulted by
//! the encoder planner before deciding to insert a new entry into the dynamic table. If the
//! predictor says no (first sighting), the planner emits a literal; if yes (likely repeat),
//! the planner invests in an insert.
//!
//! Faithful to ls-qpack's "mnemonic technique": a small circular buffer of recent
//! `nameval_hash` values. [`index`](MnemonicPredictor::index) scans the ring for the
//! current hash, writes the new hash at the cursor, and advances. Returns `true` iff the
//! scan found a prior match.
//!
//! ## Hash
//!
//! Uses std [`DefaultHasher`] (SipHash-1-3) truncated to 32 bits. Determinism caveat: the
//! algorithm behind `DefaultHasher` is documented as "not specified across releases", so
//! corpus-test byte outputs may drift slightly when the compiler is upgraded. The corpus
//! tests assert correctness (roundtrip equivalence), not exact byte counts, so this is
//! informational only. Phase 6's EMAs will further decouple the predictor from
//! byte-stable outputs anyway.
//!
//! ## False-positive floor
//!
//! The ring starts zero-initialized. A header whose `nameval_hash` happens to be `0` would
//! appear "seen" during warm-up. Probability ~1/2³² per header — negligible, not worth the
//! cost of a fill counter.

use crate::headers::qpack::{FieldLineValue, entry_name::QpackEntryName};
use std::hash::{DefaultHasher, Hasher};

/// Ring buffer size for the mnemonic predictor. ls-qpack defaults to ~28 entries; we match
/// that here. Phase 6 will replace this constant with an EMA-driven auto-tuner.
const MNEMONIC_HISTORY_SIZE: usize = 28;

/// Fixed-size circular buffer of recent `nameval_hash` values.
///
/// See the module-level docs for the algorithm. One of these lives on [`TableState`] per
/// connection; all access is already serialized by the `TableState` mutex.
///
/// [`TableState`]: super::state::TableState
#[derive(Debug)]
pub(super) struct MnemonicPredictor {
    ring: [u32; MNEMONIC_HISTORY_SIZE],
    cursor: usize,
}

impl MnemonicPredictor {
    pub(super) const fn new() -> Self {
        Self {
            ring: [0; MNEMONIC_HISTORY_SIZE],
            cursor: 0,
        }
    }

    /// Record `(name, value)` in the ring and report whether the same pair was already
    /// present. "Already present" = any of the last `MNEMONIC_HISTORY_SIZE` distinct pairs
    /// hashed to the same 32-bit value.
    pub(super) fn index(&mut self, name: &QpackEntryName<'_>, value: &FieldLineValue<'_>) -> bool {
        let h = nameval_hash(name.as_bytes(), value.as_bytes());
        let seen = self.ring.contains(&h);
        self.ring[self.cursor] = h;
        self.cursor = (self.cursor + 1) % MNEMONIC_HISTORY_SIZE;
        seen
    }
}

fn nameval_hash(name: &[u8], value: &[u8]) -> u32 {
    let mut h = DefaultHasher::new();
    h.write(name);
    // Length-agnostic separator so `"ab" + "c"` and `"a" + "bc"` don't collide.
    h.write_u8(0xff);
    h.write(value);
    // Intentional truncation — 32 bits is adequate for a 28-entry ring and keeps the ring's
    // cache footprint small. Collision probability at N=28 is ~4 × 10⁻⁸ per probe.
    #[allow(clippy::cast_possible_truncation)]
    let truncated = h.finish() as u32;
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KnownHeaderName;

    fn qen(s: &str) -> QpackEntryName<'static> {
        QpackEntryName::try_from(s.as_bytes().to_vec()).unwrap()
    }

    fn fv(s: &'static str) -> FieldLineValue<'static> {
        FieldLineValue::Static(s.as_bytes())
    }

    #[test]
    fn first_sighting_is_unseen_second_is_seen() {
        let mut p = MnemonicPredictor::new();
        assert!(!p.index(&qen("x-a"), &fv("1")));
        assert!(p.index(&qen("x-a"), &fv("1")));
    }

    #[test]
    fn different_values_under_same_name_are_independent() {
        let mut p = MnemonicPredictor::new();
        assert!(!p.index(&qen("x-a"), &fv("1")));
        assert!(!p.index(&qen("x-a"), &fv("2")));
        assert!(p.index(&qen("x-a"), &fv("1")));
        assert!(p.index(&qen("x-a"), &fv("2")));
    }

    #[test]
    fn known_header_matches_itself() {
        let mut p = MnemonicPredictor::new();
        let ct = QpackEntryName::Known(KnownHeaderName::ContentType);
        assert!(!p.index(&ct, &fv("application/json")));
        assert!(p.index(&ct, &fv("application/json")));
        assert!(!p.index(&ct, &fv("application/xml")));
    }

    #[test]
    fn ring_evicts_target_after_full_cycle_of_distinct_entries() {
        let mut p = MnemonicPredictor::new();
        // First sighting of x-target is unseen.
        assert!(!p.index(&qen("x-target"), &fv("v")));
        // Fill the ring with `MNEMONIC_HISTORY_SIZE` distinct entries — x-target's slot
        // gets overwritten along the way.
        for i in 0..MNEMONIC_HISTORY_SIZE {
            assert!(!p.index(&qen(&format!("pad-{i}")), &fv("v")));
        }
        // x-target is no longer in the ring.
        assert!(!p.index(&qen("x-target"), &fv("v")));
    }

    #[test]
    fn name_value_ordering_does_not_collide() {
        // Separator byte between name and value keeps "ab" + "c" != "a" + "bc".
        let mut p = MnemonicPredictor::new();
        assert!(!p.index(&qen("x-ab"), &fv("c")));
        assert!(!p.index(&qen("x-a"), &fv("bc")));
    }
}
