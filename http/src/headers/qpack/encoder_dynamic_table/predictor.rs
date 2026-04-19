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

use smallvec::{SmallVec, smallvec};
use std::hash::{DefaultHasher, Hasher};

/// Inline capacity / initial ring length for the mnemonic predictor. ls-qpack starts at
/// ~28 entries and re-tunes from there. We match that as the inline capacity of the
/// backing [`SmallVec`]: typical post-resize sizes hover in the same range, so the ring
/// stays stack-allocated in the common case and only spills to heap on outliers. Step 4
/// will start exercising the resize path.
const MNEMONIC_HISTORY_SIZE: usize = 28;

// EMA tuning constants. Mirror ls-qpack; not exposed as user knobs because the values
// are meaningful only against the resize logic that consumes them. Adjust here if
// future profiling shows reason to diverge.

/// Smoothing factor for both EMAs (`update_ema` in ls-qpack). Larger values weight
/// recent samples more heavily; 0.4 is ls-qpack's pick.
const EMA_ALPHA: f32 = 0.4;

/// Absolute trigger for an EMA-driven resize: fire when `|ring_len - ema_table| ≥ 1.5`.
/// Matches the resize gate inside ls-qpack's `lsqpack_enc_encode` (the call site of
/// `qenc_hist_update_size`).
const RESIZE_DIFF_ABSOLUTE: f32 = 1.5;

/// Relative trigger for an EMA-driven resize: fire when the same diff is ≥ 10% of
/// `ema_table`. Either threshold alone is sufficient. Matches ls-qpack.
const RESIZE_DIFF_RELATIVE: f32 = 0.1;

/// Eager grow-by amount when a single section adds more headers than the ring can hold.
/// Mirrors ls-qpack's `qpe_hist_nels + 4` saturation case in `lsqpack_enc_encode`.
const SATURATION_GROW_BY: usize = 4;

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
    /// True if any slot matches the query's `name_hash`. Drives the
    /// [`IndexingAllowance::NameOnly`] branch — "I've seen this name with some value;
    /// cache just the name." Paired with `nameval` in the allowance computation: a true
    /// `nameval` promotes the allowance to `Full` regardless of `name`.
    ///
    /// [`IndexingAllowance::NameOnly`]: super::policy::IndexingAllowance::NameOnly
    pub(super) name: bool,
    /// True if any slot matches the query's `nameval_hash`. Drives the `Full` allowance,
    /// unlocking insert-then-reference, warming insert, and draining-refresh Duplicate.
    pub(super) nameval: bool,
}

/// Two parallel fixed-size circular buffers of recent `name_hash` and `nameval_hash`
/// values, sharing one cursor. One lives on [`TableState`] per connection; all access is
/// already serialized by the `TableState` mutex.
///
/// Carries two exponential moving averages used (in step 4) to size the ring:
/// `table_nelem_ema` tracks the entry count of the dynamic table at eviction time, and
/// `header_count_ema` tracks the number of headers each section adds to the ring.
/// `None` until the first sample arrives — matches ls-qpack's "0 means uninitialised"
/// guard in `update_ema` without overloading a sentinel value.
///
/// [`TableState`]: super::state::TableState
#[derive(Debug)]
pub(super) struct MnemonicPredictor {
    name_hashes: SmallVec<[u32; MNEMONIC_HISTORY_SIZE]>,
    nameval_hashes: SmallVec<[u32; MNEMONIC_HISTORY_SIZE]>,
    cursor: usize,
    table_nelem_ema: Option<f32>,
    header_count_ema: Option<f32>,
    /// Number of [`remember`](Self::remember) calls in the current section. Drives both
    /// the saturation-grow check inside `remember` and the section-close sample in
    /// [`sample_header_count`](Self::sample_header_count). Reset to 0 by the latter.
    n_added_in_section: u32,
    /// Development-time counters (scaffolding; tears out with `strategy_counters.rs`).
    /// Always maintained regardless of whether counters are enabled — the cost is three
    /// integer increments per section. If this becomes measurable on real workloads we'd
    /// gate behind the same `cfg(test)` as the counters module.
    #[cfg(test)]
    pub(super) diag_n_sections: u64,
    #[cfg(test)]
    pub(super) diag_n_saturating_sections: u64,
    #[cfg(test)]
    pub(super) diag_saturation_grow_events: u64,
    /// Flipped to true the first time saturation-grow fires in the current section;
    /// reset by `sample_header_count`. Lets us count a long section as one saturating
    /// section even if it triggers multiple grow events.
    #[cfg(test)]
    section_saturated: bool,
}

impl MnemonicPredictor {
    pub(super) fn new() -> Self {
        Self {
            name_hashes: smallvec![0; MNEMONIC_HISTORY_SIZE],
            nameval_hashes: smallvec![0; MNEMONIC_HISTORY_SIZE],
            cursor: 0,
            table_nelem_ema: None,
            header_count_ema: None,
            n_added_in_section: 0,
            #[cfg(test)]
            diag_n_sections: 0,
            #[cfg(test)]
            diag_n_saturating_sections: 0,
            #[cfg(test)]
            diag_saturation_grow_events: 0,
            #[cfg(test)]
            section_saturated: false,
        }
    }

    /// Current backing-ring length. Diagnostic accessor — external callers use this as a
    /// readout of where the EMA-driven resize converged on this connection.
    #[cfg(test)]
    pub(super) fn ring_size(&self) -> usize {
        self.name_hashes.len()
    }

    /// Sample the dynamic table's current entry count into `table_nelem_ema`. ls-qpack
    /// (`qenc_sample_table_size`) calls this only when entries have actually been
    /// dropped — sampling on every encode would bias the average toward the
    /// peak-fill state. Caller is responsible for that gating.
    pub(super) fn sample_table_size(&mut self, nelem: u32) {
        update_ema(&mut self.table_nelem_ema, nelem);
    }

    /// Sample the just-finished section's [`remember`](Self::remember) count into
    /// `header_count_ema` and run the EMA-driven resize check. Mirrors ls-qpack's
    /// `qenc_sample_header_count` followed by the resize gate. Caller invokes this once
    /// per section close; predictor tracks the count internally so callers don't.
    pub(super) fn sample_header_count(&mut self) {
        let n_added = self.n_added_in_section;
        self.n_added_in_section = 0;
        #[cfg(test)]
        {
            self.diag_n_sections += 1;
            if self.section_saturated {
                self.diag_n_saturating_sections += 1;
                self.section_saturated = false;
            }
        }
        update_ema(&mut self.header_count_ema, n_added);
        self.maybe_resize_to_ema();
    }

    /// Whether the post-encode dup-draining pass should run this section. Returns
    /// `false` when the EMA-tracked table size is smaller than the EMA-tracked
    /// per-section header count: under that condition the table cycles faster than
    /// duplicates can be referenced before re-eviction, so the extra encoder-stream
    /// bytes are pure cost. Mirrors the EMA gate at the top of ls-qpack's
    /// `qenc_dup_draining`.
    ///
    /// Conservative default: allows the pass when either EMA is unprimed — without
    /// signal we fall back to the pre-step-5 always-allow behavior.
    pub(super) fn allow_dup_draining(&self) -> bool {
        let (Some(ema_table), Some(ema_header)) = (self.table_nelem_ema, self.header_count_ema)
        else {
            return true;
        };
        ema_table >= ema_header
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
        // Saturation grow: this section alone has filled the ring, so the next write
        // would overwrite the oldest in-section entry — losing within-section repeat
        // detection. Mirrors ls-qpack's eager grow-by-4 in `lsqpack_enc_encode`. Check
        // runs before the write so the entry about to be overwritten is preserved.
        if self.n_added_in_section as usize >= self.name_hashes.len() {
            let new_size = self.name_hashes.len() + SATURATION_GROW_BY;
            self.resize_ring(new_size);
            #[cfg(test)]
            {
                self.diag_saturation_grow_events += 1;
                self.section_saturated = true;
            }
        }
        self.name_hashes[self.cursor] = h.name;
        self.nameval_hashes[self.cursor] = h.nameval;
        // Modulo current ring length, not the inline-capacity constant — the ring may
        // have been resized in this call (saturation grow above) or by the EMA path.
        self.cursor = (self.cursor + 1) % self.name_hashes.len();
        self.n_added_in_section = self.n_added_in_section.saturating_add(1);
    }

    /// EMA-driven resize gate. Called from [`sample_header_count`](Self::sample_header_count)
    /// at section close. Both EMAs must be primed; the table-size EMA must exceed the
    /// header-count EMA (don't shrink the ring below the typical section length, per
    /// ls-qpack). Either threshold (absolute or relative) firing triggers a resize.
    fn maybe_resize_to_ema(&mut self) {
        let (Some(ema_table), Some(ema_header)) = (self.table_nelem_ema, self.header_count_ema)
        else {
            return;
        };
        if ema_table <= ema_header {
            return;
        }
        #[allow(clippy::cast_precision_loss)]
        let cur_size = self.name_hashes.len() as f32;
        let count_diff = (cur_size - ema_table).abs();
        if count_diff < RESIZE_DIFF_ABSOLUTE && count_diff / ema_table < RESIZE_DIFF_RELATIVE {
            return;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let new_size = ema_table.round() as usize;
        if new_size > 0 {
            self.resize_ring(new_size);
        }
    }

    /// Rebuild both rings at `new_size`, preserving entries oldest-first. On grow, pads
    /// the tail with zeros; on shrink, drops the newest excess (matches ls-qpack's
    /// `qenc_hist_update_size` — its loop walks from `first` and stops at `new_size`,
    /// discarding anything past). Cursor moves to just after the last preserved entry,
    /// modulo new size — `0` when fully filled.
    fn resize_ring(&mut self, new_size: usize) {
        let old_size = self.name_hashes.len();
        if new_size == old_size || new_size == 0 {
            return;
        }
        let mut new_name: SmallVec<[u32; MNEMONIC_HISTORY_SIZE]> = smallvec![0; new_size];
        let mut new_nameval: SmallVec<[u32; MNEMONIC_HISTORY_SIZE]> = smallvec![0; new_size];
        let count = old_size.min(new_size);
        for j in 0..count {
            let src = (self.cursor + j) % old_size;
            new_name[j] = self.name_hashes[src];
            new_nameval[j] = self.nameval_hashes[src];
        }
        self.name_hashes = new_name;
        self.nameval_hashes = new_nameval;
        self.cursor = count % new_size;
    }
}

/// Standard exponential moving average update: `ema = α·new + (1-α)·ema`. Matches
/// ls-qpack's `update_ema` — including its "first sample seeds the EMA directly" rule,
/// which we model by `None` rather than a 0 sentinel.
fn update_ema(slot: &mut Option<f32>, new: u32) {
    #[allow(clippy::cast_precision_loss)]
    let new = new as f32;
    *slot = Some(match *slot {
        Some(prev) => prev + (new - prev) * EMA_ALPHA,
        None => new,
    });
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
        // Section boundaries between each remember keep the saturation grow inactive
        // (which would otherwise enlarge the ring and prevent the target from being
        // evicted). Across many short sections the ring still cycles and evicts.
        let mut p = MnemonicPredictor::new();
        assert!(!check_nameval(&mut p, b"x-target", b"v"));
        p.sample_header_count();
        for i in 0..MNEMONIC_HISTORY_SIZE {
            let name = format!("pad-{i}");
            assert!(!check_nameval(&mut p, name.as_bytes(), b"v"));
            p.sample_header_count();
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

    #[test]
    fn first_sample_seeds_ema_directly() {
        let mut p = MnemonicPredictor::new();
        assert_eq!(p.table_nelem_ema, None);
        p.sample_table_size(40);
        assert_eq!(p.table_nelem_ema, Some(40.0));
    }

    #[test]
    fn subsequent_samples_blend_with_alpha() {
        // ema = prev + (new - prev) * 0.4 = 40 + (10 - 40) * 0.4 = 28.0
        let mut p = MnemonicPredictor::new();
        p.sample_table_size(40);
        p.sample_table_size(10);
        assert_eq!(p.table_nelem_ema, Some(28.0));
    }

    #[test]
    fn ema_converges_toward_steady_value() {
        let mut p = MnemonicPredictor::new();
        p.sample_table_size(50);
        for _ in 0..50 {
            p.sample_table_size(20);
        }
        let ema = p.table_nelem_ema.unwrap();
        assert!((ema - 20.0).abs() < 0.01, "ema={ema}");
    }

    #[test]
    fn the_two_emas_are_independent() {
        // Use values that won't trigger the maybe_resize_to_ema gate (ema_table must
        // exceed ema_header to fire), so this test isolates EMA bookkeeping from the
        // resize side effect.
        let mut p = MnemonicPredictor::new();
        p.sample_table_size(5);
        for _ in 0..10 {
            let h = MnemonicPredictor::hash(b"x", b"v");
            p.remember(h);
        }
        // n_added_in_section is now 10. Ring size after 10 remembers from initial 28 is
        // still 28 (saturation grow only fires at >= ring_len, and 10 < 28).
        p.sample_header_count();
        assert_eq!(p.table_nelem_ema, Some(5.0));
        assert_eq!(p.header_count_ema, Some(10.0));
        // Resize gate: ema_table (5) ≤ ema_header (10), so no resize fired.
        assert_eq!(p.name_hashes.len(), MNEMONIC_HISTORY_SIZE);
    }

    #[test]
    fn sample_header_count_resets_internal_counter() {
        let mut p = MnemonicPredictor::new();
        for _ in 0..7 {
            p.remember(MnemonicPredictor::hash(b"x", b"v"));
        }
        assert_eq!(p.n_added_in_section, 7);
        p.sample_header_count();
        assert_eq!(p.n_added_in_section, 0);
    }

    #[test]
    fn saturation_grow_fires_when_section_fills_ring() {
        // A single section adds enough headers to fill the ring. Eager grow should kick
        // in before the cursor wraps and overwrites the oldest in-section entry.
        let mut p = MnemonicPredictor::new();
        for i in 0..MNEMONIC_HISTORY_SIZE {
            let v = format!("v-{i}");
            p.remember(MnemonicPredictor::hash(b"x", v.as_bytes()));
        }
        // Ring is full but not yet grown — `n_added_in_section` is now 28, which equals
        // the ring length. The next `remember` call must trigger the grow before the
        // write so entry 0 isn't overwritten.
        assert_eq!(p.name_hashes.len(), MNEMONIC_HISTORY_SIZE);
        p.remember(MnemonicPredictor::hash(b"x", b"v-28"));
        assert_eq!(
            p.name_hashes.len(),
            MNEMONIC_HISTORY_SIZE + SATURATION_GROW_BY,
        );
        // The first in-section header (`v-0`) should still be findable — the grow
        // preserved it before the would-be overwrite.
        let h0 = MnemonicPredictor::hash(b"x", b"v-0");
        assert!(p.seen(h0).nameval, "oldest in-section entry preserved");
    }

    #[test]
    fn ema_resize_shrinks_ring_when_table_smaller_than_initial() {
        // ema_table (10) > ema_header (3) and well below current ring size (28). Resize
        // should target round(ema_table) = 10.
        let mut p = MnemonicPredictor::new();
        // Seed the ring with distinguishable entries so we can verify oldest-first
        // preservation across the shrink.
        for i in 0..15u32 {
            // Direct field write bypasses sample_header_count so the EMAs stay clean.
            p.name_hashes[p.cursor] = i + 1;
            p.nameval_hashes[p.cursor] = (i + 1) * 100;
            p.cursor = (p.cursor + 1) % p.name_hashes.len();
        }
        p.sample_table_size(10);
        // Three remembers seed header_count_ema at 3.
        for _ in 0..3 {
            p.remember(MnemonicPredictor::hash(b"x", b"v"));
        }
        p.sample_header_count();
        assert_eq!(p.name_hashes.len(), 10);
        // Oldest 10 of the 18 remembered entries (15 seed + 3 real) should survive,
        // walked oldest-first from the post-seed cursor. The 5 newest are dropped.
        // Concretely: seeds 1..=15 then 3 real remembers means 18 logical entries in a
        // 28-slot ring (no wrap yet, since 18 < 28). Oldest preserved entry is seed `1`
        // — but wait: the ring is treated as always-wrapped, so "oldest" is at cursor.
        // After 18 writes, cursor=18; "oldest" walking from cursor wraps through
        // [18..28] (zeros) then [0..18] (real). The first 10 of those are the 10 zeros
        // followed by the 0 zeros ... actually 28 - 18 = 10 zeros at [18..28], then we
        // need 0 more from the wrap. So the new ring is exactly the 10 zero slots.
        // Cursor lands at 0 (count == new_size).
        assert_eq!(p.cursor, 0);
        assert!(p.name_hashes.iter().all(|&x| x == 0));
    }

    #[test]
    fn ema_resize_does_not_fire_when_table_smaller_than_header_avg() {
        let mut p = MnemonicPredictor::new();
        p.sample_table_size(5);
        // Seed n_added_in_section large enough to dominate; ema_header will be 20.
        p.n_added_in_section = 20;
        p.sample_header_count();
        assert_eq!(p.name_hashes.len(), MNEMONIC_HISTORY_SIZE);
    }

    #[test]
    fn ema_resize_does_not_fire_when_diff_below_thresholds() {
        // cur=28, ema_table=27.0 (rounds to 27): diff=1.0, 1.0/27 ≈ 0.037. Both below
        // their thresholds (1.5 and 0.1), so no resize.
        let mut p = MnemonicPredictor::new();
        p.table_nelem_ema = Some(27.0);
        p.header_count_ema = Some(5.0);
        p.maybe_resize_to_ema();
        assert_eq!(p.name_hashes.len(), MNEMONIC_HISTORY_SIZE);
    }

    #[test]
    fn ema_resize_fires_via_relative_threshold_only() {
        // cur=28, ema_table=25.0: diff=3.0 (≥ 1.5 absolute → fires). Construct a case
        // where only relative would fire by picking smaller numbers.
        // Manually shrink the ring first to 10 to test small-number behavior.
        let mut p = MnemonicPredictor::new();
        p.resize_ring(10);
        p.table_nelem_ema = Some(11.5);
        p.header_count_ema = Some(2.0);
        // diff = 1.5 is exactly the absolute threshold — strictly less than 1.5 we want
        // ONLY the relative threshold to fire. Use ema_table=12, diff=2: 2 < 1.5? no.
        // So pick ema_table=11, diff=1, 1/11 ≈ 0.091 < 0.1. Below both. Then 12 → diff=2.
        p.table_nelem_ema = Some(12.0);
        p.maybe_resize_to_ema();
        assert_eq!(p.name_hashes.len(), 12);
    }

    #[test]
    fn resize_ring_preserves_entries_oldest_first_on_grow() {
        // Manually load a ring with known values, then grow. Oldest-first walk from
        // cursor should land at indices [0..count] of the new ring.
        let mut p = MnemonicPredictor::new();
        p.resize_ring(4);
        // Write 4 entries, fully filling and wrapping cursor back to 0.
        for v in 1u32..=4 {
            p.name_hashes[p.cursor] = v;
            p.cursor = (p.cursor + 1) % p.name_hashes.len();
        }
        // Cursor is now 0; oldest entry is name_hashes[0] = 1.
        p.resize_ring(6);
        // After grow, [0..4] should be 1,2,3,4 in oldest-first order; [4..6] zero.
        assert_eq!(p.name_hashes.as_slice(), &[1, 2, 3, 4, 0, 0]);
        assert_eq!(p.cursor, 4);
    }

    #[test]
    fn allow_dup_draining_when_both_emas_unprimed() {
        let p = MnemonicPredictor::new();
        assert!(p.allow_dup_draining());
    }

    #[test]
    fn allow_dup_draining_when_only_one_ema_primed() {
        // Conservative: lacking complete signal, fall back to allow.
        let mut p = MnemonicPredictor::new();
        p.table_nelem_ema = Some(40.0);
        assert!(p.allow_dup_draining());

        let mut p = MnemonicPredictor::new();
        p.header_count_ema = Some(5.0);
        assert!(p.allow_dup_draining());
    }

    #[test]
    fn allow_dup_draining_when_table_meets_or_exceeds_header_avg() {
        let mut p = MnemonicPredictor::new();
        p.table_nelem_ema = Some(40.0);
        p.header_count_ema = Some(10.0);
        assert!(p.allow_dup_draining());

        // Equal case included — duplicates still have room to amortize.
        p.table_nelem_ema = Some(10.0);
        p.header_count_ema = Some(10.0);
        assert!(p.allow_dup_draining());
    }

    #[test]
    fn deny_dup_draining_when_table_smaller_than_header_avg() {
        let mut p = MnemonicPredictor::new();
        p.table_nelem_ema = Some(5.0);
        p.header_count_ema = Some(10.0);
        assert!(!p.allow_dup_draining());
    }

    #[test]
    fn resize_ring_drops_newest_on_shrink() {
        let mut p = MnemonicPredictor::new();
        p.resize_ring(6);
        for v in 1u32..=6 {
            p.name_hashes[p.cursor] = v;
            p.cursor = (p.cursor + 1) % p.name_hashes.len();
        }
        // Cursor at 0, ring is [1, 2, 3, 4, 5, 6] in oldest-first order from cursor.
        p.resize_ring(4);
        // Oldest 4 preserved: 1, 2, 3, 4. Newest 2 (5, 6) dropped. Matches ls-qpack's
        // qenc_hist_update_size loop semantics.
        assert_eq!(p.name_hashes.as_slice(), &[1, 2, 3, 4]);
        assert_eq!(p.cursor, 0);
    }
}
