//! Development-time strategy counters for the QPACK encoder.
//!
//! Tallies which per-line program the planner picked across an encode run, so we can
//! compare which strategies fire at what rates under different configs — and what the
//! effect of a policy change is on the strategy mix, not just on the total byte count.
//!
//! Scaffolding only: gated to `cfg(test)` so it's absent from release builds. Intended to
//! be torn out before the dynamic-table branch ships. The corpus test (the only call site)
//! opts in via [`EncoderDynamicTable::enable_strategy_counters`].
//!
//! [`EncoderDynamicTable::enable_strategy_counters`]:
//! super::EncoderDynamicTable::enable_strategy_counters

#![cfg(test)]

use super::policy::{
    BudgetCtx, DynRef, EncStreamAction, EncoderProgram, HeaderBlockAction, IndexingAllowance,
    MatchState,
};

/// Per-connection tally of which planner strategy fired per header line. One instance
/// lives on [`TableState`](super::state::TableState) when counters are enabled; the corpus
/// test drains and aggregates them after each qif.
///
/// Bucket design tracks the three gaps identified in the phase-8 review:
/// - Gap #1 (warming insert on dyn-name match): the three `warming_insert_*` buckets, split
///   by what name ref was available at the moment the warming insert fired.
/// - Gap #2 (main-path DUP threshold): `main_path_dup_refresh` — count of main-path
///   refreshes. Compare against `indexed_dynamic_existing` (fresh references) to see how
///   often the draining-refresh branch fires vs the fast path.
/// - Gap #3 (name-only insert requires slot): `name_only_insert` vs `name_only_skipped_no_slot`
///   — ratio shows how often the predictor's `NameOnly` signal goes unused because the
///   current implementation gates it on a blocking slot.
///
/// Plus baseline buckets for context: plain indexed refs, static, insert-then-ref, the
/// three literal fallthroughs, and the two guard-fire counters.
#[derive(Debug, Default, Clone, Copy)]
pub(in crate::headers::qpack) struct StrategyCounters {
    /// `IndexedDynamic(Existing)` — a live dynamic-table entry is referenced directly.
    /// Includes both fresh and draining fast-path hits (draining-refresh goes into
    /// `main_path_dup_refresh` instead).
    pub(in crate::headers::qpack) indexed_dynamic_existing: u64,
    /// `IndexedStatic` — a static-table full match.
    pub(in crate::headers::qpack) indexed_static: u64,
    /// `Insert + IndexedDynamic(NewlyInserted)` — insert-then-reference in the same section.
    pub(in crate::headers::qpack) insert_then_reference: u64,
    /// `Duplicate + IndexedDynamic(Existing)` — the draining-refresh branch. Emits a
    /// background Duplicate on the encoder stream and references the still-live original.
    pub(in crate::headers::qpack) main_path_dup_refresh: u64,
    /// `Insert + LiteralLiteralName` — warming insert with no name ref available.
    pub(in crate::headers::qpack) warming_insert_no_match: u64,
    /// `Insert + LiteralStaticNameRef` — warming insert using a static name ref in the
    /// section.
    pub(in crate::headers::qpack) warming_insert_static_name: u64,
    /// `Insert + LiteralDynamicNameRef` — warming insert using a dynamic name ref in the
    /// section. **This is the gap-#1 bucket**: ls-qpack does not emit this shape; it
    /// emits a plain `LiteralDynamicNameRef` with no insert when a dyn name match exists.
    pub(in crate::headers::qpack) warming_insert_dyn_name: u64,
    /// `InsertNameOnly + LiteralDynamicNameRef(NewlyInserted)` — name-only insert with
    /// in-section ref (requires a blocking slot today).
    pub(in crate::headers::qpack) name_only_insert: u64,
    /// The predictor flagged `NameOnly`, no match was available, and no blocking slot was
    /// either. Trillium falls through to `LiteralLiteralName`; ls-qpack's !risk branch
    /// would have emitted `InsertNameOnly + LiteralLiteralName` (a warming name-only
    /// insert). **This is the gap-#3 bucket.**
    pub(in crate::headers::qpack) name_only_skipped_no_slot: u64,
    /// Successful Duplicate emissions from the post-encode
    /// [`dup_draining_pass`](super::state::TableState::dup_draining_pass). A section may
    /// produce multiple; each is counted separately.
    pub(in crate::headers::qpack) dup_draining_pass_emits: u64,
    /// `None + LiteralStaticNameRef` — plain literal with static name ref (no insert).
    pub(in crate::headers::qpack) literal_static_name: u64,
    /// `None + LiteralDynamicNameRef` — plain literal with dynamic name ref (no insert).
    pub(in crate::headers::qpack) literal_dyn_name: u64,
    /// `None + LiteralLiteralName` — plain literal with literal name (no insert).
    pub(in crate::headers::qpack) literal_literal_name: u64,
    /// Phase-5 inflation guard fired and re-planned the line with indexing disabled.
    pub(in crate::headers::qpack) inflation_retry: u64,
    /// Phase-7 per-encode insert budget gated the line (zeroed `MatchState` dyn fields
    /// and forced `IndexingAllowance::None` before `select_program`).
    pub(in crate::headers::qpack) insert_budget_blocked: u64,
    /// Per-line distribution of the [`IndexingAllowance`] the planner computed from the
    /// mnemonic predictor. `None` means neither the name nor the nameval was in the ring
    /// (or the header was sensitive); `NameOnly` means only the name was seen; `Full`
    /// means the nameval was seen. Tells us whether the predictor is the bottleneck on
    /// insertion — if the vast majority of lines are `None`, few lines are even eligible
    /// for insert-then-reference or warming insert.
    pub(in crate::headers::qpack) allowance_none: u64,
    pub(in crate::headers::qpack) allowance_name_only: u64,
    pub(in crate::headers::qpack) allowance_full: u64,
    /// Total sections planned on this connection. Paired with `saturating_sections` to
    /// read the fraction of sections whose length exceeds the ring at section start.
    pub(in crate::headers::qpack) n_sections: u64,
    /// Sections during which at least one saturation-grow event fired (i.e. the section
    /// was longer than the ring at section start, triggering an eager grow). If this is
    /// a large fraction of `n_sections`, the ring is sized too small for the typical
    /// section length.
    pub(in crate::headers::qpack) n_saturating_sections: u64,
    /// Total saturation-grow events across the run. One saturating section can produce
    /// multiple events if it's long enough.
    pub(in crate::headers::qpack) saturation_grow_events: u64,
    /// Final ring size on this connection (what the EMA resize path converged to). If
    /// this stays at the default of 28 the EMA gate (`ema_table > ema_header`) likely
    /// isn't firing.
    pub(in crate::headers::qpack) final_ring_size: u64,
}

impl StrategyCounters {
    /// Merge `other` into `self`, field by field. Used by the corpus test to aggregate
    /// per-qif counters into per-config totals.
    pub(in crate::headers::qpack) fn add(&mut self, other: &Self) {
        self.indexed_dynamic_existing += other.indexed_dynamic_existing;
        self.indexed_static += other.indexed_static;
        self.insert_then_reference += other.insert_then_reference;
        self.main_path_dup_refresh += other.main_path_dup_refresh;
        self.warming_insert_no_match += other.warming_insert_no_match;
        self.warming_insert_static_name += other.warming_insert_static_name;
        self.warming_insert_dyn_name += other.warming_insert_dyn_name;
        self.name_only_insert += other.name_only_insert;
        self.name_only_skipped_no_slot += other.name_only_skipped_no_slot;
        self.dup_draining_pass_emits += other.dup_draining_pass_emits;
        self.literal_static_name += other.literal_static_name;
        self.literal_dyn_name += other.literal_dyn_name;
        self.literal_literal_name += other.literal_literal_name;
        self.inflation_retry += other.inflation_retry;
        self.insert_budget_blocked += other.insert_budget_blocked;
        self.allowance_none += other.allowance_none;
        self.allowance_name_only += other.allowance_name_only;
        self.allowance_full += other.allowance_full;
        self.n_sections += other.n_sections;
        self.n_saturating_sections += other.n_saturating_sections;
        self.saturation_grow_events += other.saturation_grow_events;
        // Ring size is a final-value snapshot per connection — sum across connections so
        // the aggregated value is an average-weighted-by-connection proxy. For the corpus
        // test, one connection per qif, so the aggregate sum divided by n qifs gives the
        // mean final ring size.
        self.final_ring_size += other.final_ring_size;
    }
}

/// Classify the (match_state, indexing, budget, program) tuple from one
/// `plan_with_indexing` iteration and bump the corresponding counter. Called from the
/// planner after [`select_program`](super::policy::select_program) picks the program for
/// one header line.
///
/// Detects the gap-#3 "name-only skipped because no blocking slot" case by inspecting the
/// program's fallthrough-to-literal shape together with the input `indexing` and `budget`.
pub(super) fn classify_program(
    match_state: &MatchState,
    indexing: IndexingAllowance,
    budget: &BudgetCtx,
    program: &EncoderProgram,
    counters: &mut StrategyCounters,
) {
    match indexing {
        IndexingAllowance::None => counters.allowance_none += 1,
        IndexingAllowance::NameOnly => counters.allowance_name_only += 1,
        IndexingAllowance::Full => counters.allowance_full += 1,
    }
    match (program.enc_stream, program.header_block) {
        (EncStreamAction::None, HeaderBlockAction::IndexedDynamic(_)) => {
            counters.indexed_dynamic_existing += 1;
        }
        (EncStreamAction::None, HeaderBlockAction::IndexedStatic(_)) => {
            counters.indexed_static += 1;
        }
        (EncStreamAction::Insert, HeaderBlockAction::IndexedDynamic(DynRef::NewlyInserted)) => {
            counters.insert_then_reference += 1;
        }
        (EncStreamAction::Duplicate(_), HeaderBlockAction::IndexedDynamic(_)) => {
            counters.main_path_dup_refresh += 1;
        }
        (EncStreamAction::Insert, HeaderBlockAction::LiteralStaticNameRef { .. }) => {
            counters.warming_insert_static_name += 1;
        }
        (EncStreamAction::Insert, HeaderBlockAction::LiteralDynamicNameRef { .. }) => {
            counters.warming_insert_dyn_name += 1;
        }
        (EncStreamAction::Insert, HeaderBlockAction::LiteralLiteralName) => {
            counters.warming_insert_no_match += 1;
        }
        (EncStreamAction::InsertNameOnly, HeaderBlockAction::LiteralDynamicNameRef { .. }) => {
            counters.name_only_insert += 1;
        }
        (EncStreamAction::None, HeaderBlockAction::LiteralStaticNameRef { .. }) => {
            counters.literal_static_name += 1;
        }
        (EncStreamAction::None, HeaderBlockAction::LiteralDynamicNameRef { .. }) => {
            counters.literal_dyn_name += 1;
        }
        (EncStreamAction::None, HeaderBlockAction::LiteralLiteralName) => {
            counters.literal_literal_name += 1;
            // Gap #3 detection: a NameOnly allowance that couldn't fire the name-only
            // insert branch because no other match was available *and* no blocking slot
            // was either. Mirrors the name-only branch's gating conditions in
            // `select_program`.
            if matches!(indexing, IndexingAllowance::NameOnly)
                && match_state.dynamic_full.is_none()
                && match_state.static_full.is_none()
                && match_state.static_name.is_none()
                && match_state.dynamic_name.is_none()
                && !budget.can_take_blocking_slot()
            {
                counters.name_only_skipped_no_slot += 1;
            }
        }
        // All remaining combinations would be planner bugs (e.g. `Insert` with an indexed
        // static header block). Debug builds already assert the valid-pair invariants in
        // `plan_with_indexing`; here we silently ignore so counters never panic in release
        // test runs.
        _ => {}
    }
}
