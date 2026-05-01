//! Dynamic-table-aware QPACK field section encoder (RFC 9204 §4.5).
//!
//! Two-phase design:
//!
//! 1. **Plan phase** runs under the `TableState` mutex. It walks the field section, decides each
//!    line's encoding (static ref / dynamic ref / literal, with optional warming insert),
//!    accumulates a list of [`Emission`]s, and updates the section's Required Insert Count and
//!    min-referenced `abs_idx`. Section registration happens inside the same critical section so
//!    the blocked-streams accounting is atomic.
//!
//! 2. **Emit phase** runs lock-free. It converts the planned [`Emission`] list into wire bytes
//!    using the §4.5 wire helpers.
//!
//! ## Per-line decision
//!
//! Direct chain inside [`Planner::plan_header_line`]:
//!
//! 1. **Static full match** — `IndexedStatic`, the cheapest possible encoding.
//! 2. **Dynamic full match** — `IndexedDynamic` if the entry is referenceable (already-acked, or
//!    this section is permitted to block).
//! 3. **Warming insert** — when the predictor reports we've seen this `(name, value)` pair before.
//!    Insert the entry now so future sections can `IndexedDynamic` against it; this section emits a
//!    literal regardless. We never reference a freshly-inserted entry in the same section.
//! 4. **Literal form** — `LiteralStaticNameRef` → `LiteralDynamicNameRef` (when the pre-insert
//!    dyn-name lookup is still live and the budget allows) → `LiteralLiteralName`.
//!
//! ## Blocking budget
//!
//! Captured once at planner construction: `can_block_section = is_stream_blocking ||
//! currently_blocked_streams < max_blocked_streams`. Stable for the duration of the section
//! because we hold the `TableState` lock the whole time.
//!
//! Per-line, a dynamic reference is allowed iff `abs_idx < KRC || can_block_section` —
//! already-acked refs are always free, unacked refs are gated by the snapshot.
//!
//! ## Sensitive headers
//!
//! Headers whose name marks the value uncacheable
//! ([`EntryName::has_uncacheable_value`]) are excluded from the predictor and never
//! warm-inserted. This is a conservative stand-in for the RFC 9204 §4.5.4 N bit until
//! `FieldLine` carries the bit through end-to-end.

use super::{EncoderDynamicTable, SectionRefs, state::TableState};
use crate::headers::{
    entry_name::EntryName,
    qpack::{
        FieldLineValue, FieldSection, HeaderObserver,
        instruction::field_section::{FieldLineInstruction, FieldSectionPrefix},
        static_table::static_table_lookup,
    },
    recent_pairs::RecentPairs,
    static_hit::StaticHit,
};

/// Saturating `usize` → `u32` conversion. Wire byte sizes never meaningfully exceed
/// `u32::MAX`; clamping at the boundary keeps the metrics arithmetic honest without
/// adding a panic on pathological inputs.
fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl EncoderDynamicTable {
    /// Encode `field_section` onto `buf` for transmission on `stream_id`.
    ///
    /// Delegates to [`encode_field_lines`](Self::encode_field_lines) after extracting the
    /// ordered `(name, value)` list from `field_section`.
    pub(crate) fn encode(
        &self,
        field_section: &FieldSection<'_>,
        buf: &mut Vec<u8>,
        stream_id: u64,
    ) {
        self.encode_field_lines(&field_section.field_lines(), buf, stream_id);
    }

    /// Encode a pre-ordered list of field lines onto `buf` for transmission on `stream_id`.
    ///
    /// Exists separately from [`encode`](Self::encode) because `FieldSection` stores unknown
    /// header names in a `HashMap`, whose iteration order is non-deterministic. Callers that
    /// need a stable wire encoding for the same logical input (e.g. the encoder corpus test
    /// which measures compression run-to-run) construct the ordered `field_lines` themselves
    /// and bypass the `FieldSection` → `field_lines()` conversion.
    pub(in crate::headers) fn encode_field_lines(
        &self,
        field_lines: &[(EntryName<'_>, FieldLineValue<'_>, bool)],
        buf: &mut Vec<u8>,
        stream_id: u64,
    ) {
        let plan;
        let max_capacity;
        let made_inserts;
        let enc_stream_bytes;
        let krc_at_encode;
        {
            let mut state = self.state.lock().unwrap();
            max_capacity = state.max_capacity;
            krc_at_encode = state.known_received_count;
            let mut planner = Planner::new(&mut state, stream_id, &self.observer);
            for (name, value, never_indexed) in field_lines {
                planner.plan_header_line(name, value.reborrow(), *never_indexed);
            }
            made_inserts = planner.made_inserts;
            enc_stream_bytes = planner.enc_stream_bytes;
            plan = planner.finish();
            if plan.section_ric > 0 {
                state
                    .outstanding_sections
                    .entry(stream_id)
                    .or_default()
                    .push_back(SectionRefs {
                        required_insert_count: plan.section_ric,
                        min_ref_abs_idx: plan.min_ref_abs_idx,
                    });
            }
        }

        // Wake the encoder-stream writer if the plan enqueued any inserts.
        if made_inserts {
            self.event.notify(usize::MAX);
        }

        let section_start = buf.len();
        emit_section_prefix(plan.section_ric, max_capacity, buf);
        for emission in &plan.emissions {
            emit(emission, plan.section_ric, buf);
        }

        let header_block_bytes = u32::try_from(buf.len() - section_start).unwrap_or(u32::MAX);

        // Research-mode observer/priming metrics (see
        // `encoder_dynamic_table::connection_metrics`). Field-section bytes are
        // load-bearing for response latency; encoder-stream inserts enqueued here are
        // load-bearing too (they gate the client's decoder on this section). Priming
        // bytes (eager) were accounted separately in `initialize_from_peer_settings`.
        let dynamic_refs: Vec<u64> = plan
            .emissions
            .iter()
            .filter_map(|emission| match emission {
                Emission::IndexedDynamicPreBase(abs_idx)
                | Emission::LiteralDynamicNameRefPreBase { abs_idx, .. } => Some(*abs_idx),
                _ => None,
            })
            .collect();
        self.metrics.record_section(
            header_block_bytes,
            enc_stream_bytes,
            &dynamic_refs,
            krc_at_encode,
        );
    }
}

/// A single field-line encoding decision produced by the plan phase.
///
/// Borrows name/value slices from the caller's `FieldSection` — the plan lives only for the
/// duration of one `encode()` call, so the lifetime is always trivially satisfied.
///
/// The literal variants carry a `never_indexed` flag (RFC 9204 §4.5.4 N bit). The flag is
/// sourced from the per-value `HeaderValue::is_never_indexed()` set by the QPACK decoder
/// when re-emitting a section round-tripped through `Headers` (e.g. through trillium-proxy).
#[derive(Debug)]
enum Emission<'lines, 'names> {
    /// §4.5.2: Indexed Field Line referencing the QPACK static table (T=1).
    IndexedStatic(u8),

    /// §4.5.2: Indexed Field Line referencing the dynamic table (T=0), pre-base. Stored as
    /// the absolute table index; converted to the relative index at emit time.
    IndexedDynamicPreBase(u64),

    /// §4.5.4: Literal Field Line with Name Reference, against the QPACK static table (T=1).
    LiteralStaticNameRef {
        name_index: u8,
        value: FieldLineValue<'lines>,
        never_indexed: bool,
    },

    /// §4.5.4: Literal Field Line with Name Reference, against the dynamic table (T=0),
    /// pre-base. Stored as the absolute table index; converted to the relative index at emit
    /// time.
    LiteralDynamicNameRefPreBase {
        abs_idx: u64,
        value: FieldLineValue<'lines>,
        never_indexed: bool,
    },

    /// §4.5.6: Literal Field Line with Literal Name.
    LiteralLiteralName {
        name: &'lines EntryName<'names>,
        value: FieldLineValue<'lines>,
        never_indexed: bool,
    },
}

/// Output of the plan phase.
struct Plan<'lines, 'names> {
    emissions: Vec<Emission<'lines, 'names>>,
    section_ric: u64,
    min_ref_abs_idx: Option<u64>,
}

/// Planning-phase state. Holds a mutable reference to `TableState` and accumulates
/// decisions as the field section is walked. Runs entirely under the `TableState` lock.
struct Planner<'state, 'lines, 'names> {
    state: &'state mut TableState,
    /// Cross-connection header observer, consulted by the dup-drain refresh pass to
    /// decide whether an about-to-be-pressed-off tail entry is still hot enough to
    /// warrant a Duplicate instruction.
    observer: &'state HeaderObserver,
    emissions: Vec<Emission<'lines, 'names>>,
    section_ric: u64,
    min_ref_abs_idx: Option<u64>,
    /// True iff this section may take dynamic references whose `abs_idx >= KRC`. Snapshot
    /// at planner construction; doesn't change mid-section because we hold the
    /// `TableState` lock throughout planning. Either the stream is already blocking (its
    /// slot is already counted) or a fresh slot is available under the peer's
    /// `SETTINGS_QPACK_BLOCKED_STREAMS`.
    can_block_section: bool,
    /// Set when the plan phase has enqueued at least one encoder-stream insert. The caller
    /// uses this flag after dropping the lock to notify the encoder-stream writer.
    made_inserts: bool,
    /// Running total of encoder-stream bytes enqueued during planning. Consumed by the
    /// per-section research-mode metrics in [`ConnectionMetrics::record_section`].
    ///
    /// [`ConnectionMetrics::record_section`]: super::connection_metrics::ConnectionMetrics::record_section
    enc_stream_bytes: u32,
}

impl<'state, 'lines, 'names> Planner<'state, 'lines, 'names> {
    fn new(
        state: &'state mut TableState,
        stream_id: u64,
        observer: &'state HeaderObserver,
    ) -> Self {
        let can_block_section = state.is_stream_blocking(stream_id)
            || state.currently_blocked_streams() < state.max_blocked_streams;
        Self {
            state,
            observer,
            emissions: Vec::new(),
            section_ric: 0,
            min_ref_abs_idx: None,
            can_block_section,
            made_inserts: false,
            enc_stream_bytes: 0,
        }
    }

    fn finish(self) -> Plan<'lines, 'names> {
        Plan {
            emissions: self.emissions,
            section_ric: self.section_ric,
            min_ref_abs_idx: self.min_ref_abs_idx,
        }
    }

    /// Plan a single field line — decide encoding and apply any encoder-stream side effect.
    fn plan_header_line(
        &mut self,
        name: &'lines EntryName<'names>,
        value: FieldLineValue<'lines>,
        never_indexed: bool,
    ) {
        // Cross-connection observer accumulator — fold-on-close, no shared-state
        // mutation here. See `header_observer` module docs. Never-indexed values are
        // skipped entirely to keep them out of the cross-connection priming pool: a
        // `HeaderValue::const_new(...)` literal would otherwise reach `observe` as a
        // `Static` provenance and end up tracked as a primed pair.
        if !never_indexed {
            self.state.accum.observe(name, &value);
        }

        // Pre-decision: hash for the recent-pairs ring (sensitive headers are excluded
        // so they never enter the ring). Computed once and threaded through both `seen`
        // (read for the indexing decision) and `remember` (write at the end of planning).
        // A never-indexed value is treated like a sensitive header: never hashed, never
        // tracked, never inserted.
        let uncacheable = name.has_uncacheable_value() || never_indexed;
        let hash = (!uncacheable).then(|| RecentPairs::hash(name.as_bytes(), value.as_bytes()));

        // Indexing decision: a non-sensitive header is eligible for warm insertion the
        // second (and subsequent) time the encoder sees it on this connection.
        let should_index = hash.is_some_and(|h| self.state.recent_pairs.seen(h));

        let emission = self.plan_emission(name, value, should_index, never_indexed);
        self.emissions.push(emission);

        // Defer the ring write until after planning — a header is never visible to its
        // own earlier `seen` query.
        if let Some(h) = hash {
            self.state.recent_pairs.remember(h);
        }
    }

    /// The per-line decision chain. Mutates `self` (records refs, possibly inserts a new
    /// entry, accumulates wire bytes).
    fn plan_emission(
        &mut self,
        name: &'lines EntryName<'names>,
        value: FieldLineValue<'lines>,
        should_index: bool,
        never_indexed: bool,
    ) -> Emission<'lines, 'names> {
        let static_match = static_table_lookup(name, Some(value.as_bytes()));

        // 1. Static full match: cheapest possible encoding, no dynamic-table interaction. RFC 9204
        //    §4.5.4 requires N=1 fields to use a literal representation, so we skip the indexed
        //    shortcut when never_indexed.
        if !never_indexed && let StaticHit::Full(i) = static_match {
            return Emission::IndexedStatic(i);
        }

        let name_lookup = self.state.by_name.get(name);

        // 2. Dynamic full match: reference directly when budget allows. Pre-insert lookup — we
        //    never reference an entry that the upcoming warming insert would create in this same
        //    section. Same N=1 literal-only rule as step 1.
        let dyn_full = name_lookup.and_then(|i| i.by_value.get(value.as_bytes()).copied());
        if !never_indexed
            && let Some(abs_idx) = dyn_full
            && self.can_ref(abs_idx)
        {
            self.record_ref(abs_idx);
            return Emission::IndexedDynamicPreBase(abs_idx);
        }

        // 3. Pre-insert dyn-name lookup. Captured before the warming insert so the literal-form
        //    picker uses the existing entry rather than the freshly-inserted one (which would
        //    amount to insert-then-reference; we don't do that). Validity is re-checked post-insert
        //    via `entry_at_abs` because the warming insert may evict older entries to make room.
        let dyn_name_pre = name_lookup.map(|i| i.latest_any);

        // 4. Warming insert: predictor saw this pair (or eager mode). Seeds a future- section
        //    reference; emission for *this* section is always a literal. Failure is silently
        //    ignored — we just fall through to the literal form picker below. Before the insert,
        //    give the dup-drain pass a chance to refresh a hot tail entry that's at risk of
        //    eviction.
        if should_index {
            self.refresh_hot_tail();
            let pre_count = self.state.pending_ops.len();
            let _ = self
                .state
                .insert(name.reborrow(), value.reborrow(), self.min_ref_abs_idx);
            if self.state.pending_ops.len() > pre_count {
                self.made_inserts = true;
                if let Some(wire) = self.state.pending_ops.back() {
                    self.enc_stream_bytes =
                        self.enc_stream_bytes.saturating_add(to_u32(wire.len()));
                }
            }
        }

        // 5. Literal form: static name ref → pre-insert dyn name ref (still live and referenceable)
        //    → literal-literal. Section never references the freshly- inserted entry from step 4. A
        //    `StaticHit::Full` reaches step 5 only when `never_indexed` blocked the §4.5.2 indexed
        //    shortcut; the static index is still a valid name reference for §4.5.4.
        let static_name_index = match static_match {
            StaticHit::Name(i) => Some(i),
            StaticHit::Full(i) if never_indexed => Some(i),
            _ => None,
        };
        if let Some(i) = static_name_index {
            return Emission::LiteralStaticNameRef {
                name_index: i,
                value,
                never_indexed,
            };
        }
        if let Some(abs_idx) = dyn_name_pre
            && self.state.entry_at_abs(abs_idx).is_some()
            && self.can_ref(abs_idx)
        {
            self.record_ref(abs_idx);
            return Emission::LiteralDynamicNameRefPreBase {
                abs_idx,
                value,
                never_indexed,
            };
        }
        Emission::LiteralLiteralName {
            name,
            value,
            never_indexed,
        }
    }

    /// If the table is close enough to full that the oldest entry is at risk of being
    /// evicted by an upcoming warming insert, and the observer still considers it hot,
    /// emit a Duplicate so the next section can still reference it.
    ///
    /// ## Gate
    ///
    /// Fires only when `headroom < primed_bytes` — i.e. the remaining room is less than
    /// the initial priming footprint. With no priming, `primed_bytes` is 0 and the gate
    /// never opens, which cleanly disables dup-drain for connections where the structural
    /// hot-at-the-tail shape doesn't apply.
    ///
    /// Early-refreshing at this threshold (rather than "right before eviction") preserves
    /// the ability to duplicate at all: a Duplicate of the oldest entry needs enough room
    /// to add its own size, and that's impossible once the table is already full — the
    /// source entry is its own pin, so nothing older can be evicted to make room.
    ///
    /// ## Budget
    ///
    /// At most one attempted Duplicate per warming insert. In a burst of N warming
    /// inserts, up to N refresh attempts fire — each cycles the (then-)oldest hot entry
    /// to the head, so a sustained burst naturally drains the full priming list to the
    /// front without an explicit counter.
    ///
    /// ## Failure
    ///
    /// Silently dropped. The most likely reason is "not enough headroom to add another
    /// entry" (the dup needs its own size free), which is already the worst-case we
    /// designed the gate to avoid — if we hit it anyway, the warming insert still
    /// proceeds and evicts the oldest entry normally.
    fn refresh_hot_tail(&mut self) {
        if self.state.primed_bytes == 0 {
            return;
        }
        let headroom = self.state.capacity.saturating_sub(self.state.current_size);
        if headroom >= self.state.primed_bytes {
            return;
        }

        let (oldest_abs, name, value_opt) = {
            let Some(oldest) = self.state.entries.back() else {
                return;
            };
            let oldest_abs = self
                .state
                .insert_count
                .saturating_sub(self.state.entries.len() as u64);
            let name = oldest.name.clone();
            let value_opt = if oldest.value.is_empty() {
                None
            } else {
                Some(FieldLineValue::Owned(oldest.value.to_vec()))
            };
            (oldest_abs, name, value_opt)
        };

        if !self.observer.is_hot(&name, value_opt.as_ref()) {
            return;
        }

        let pre_count = self.state.pending_ops.len();
        if self
            .state
            .duplicate(oldest_abs, self.min_ref_abs_idx)
            .is_ok()
            && self.state.pending_ops.len() > pre_count
        {
            self.made_inserts = true;
            if let Some(wire) = self.state.pending_ops.back() {
                self.enc_stream_bytes = self.enc_stream_bytes.saturating_add(to_u32(wire.len()));
            }
        }
    }

    /// True iff a header-block reference to `abs_idx` is allowed under the section's
    /// blocking budget. Already-acked entries (`abs_idx < KRC`) are always referenceable;
    /// unacked entries require the section to be permitted to block.
    fn can_ref(&self, abs_idx: u64) -> bool {
        abs_idx < self.state.known_received_count || self.can_block_section
    }

    fn record_ref(&mut self, abs_idx: u64) {
        let needed_ric = abs_idx + 1;
        if needed_ric > self.section_ric {
            self.section_ric = needed_ric;
        }
        self.min_ref_abs_idx = Some(match self.min_ref_abs_idx {
            Some(cur) => cur.min(abs_idx),
            None => abs_idx,
        });
    }
}

/// Write the section prefix (§4.5.1). For a section with no dynamic references this is the
/// fixed two-byte `[0x00, 0x00]` prefix. Otherwise it encodes the Required Insert Count per
/// §4.5.1.1 and a Delta Base of zero (base = RIC, sign = 0) per §4.5.1.2.
///
/// **Always `base = RIC` — post-base references unimplemented.** Setting `base < RIC` would
/// let us emit references in `[base, RIC)` via the post-base instruction shapes (§4.5.4 +
/// §4.5.5), which yields smaller varints than the pre-base form when many references sit
/// near the section's RIC. The win is meaningful only for high-cardinality sections (e.g.
/// reverse-proxy traffic forwarding many distinct headers per request); typical
/// request/response flows don't benefit. Decoder side already supports both shapes
/// (`apply_indexed_with_post_base` etc.), so adding encoder-side post-base emission is a
/// pure wire-efficiency optimization, not a correctness or interop gap. Revisit if a
/// proxy-workload profiler shows headroom; until then `base = RIC` keeps the encoder
/// simpler with no behavior cost in current workloads.
fn emit_section_prefix(section_ric: u64, max_capacity: usize, buf: &mut Vec<u8>) {
    let encoded_required_insert_count = if section_ric == 0 {
        0
    } else {
        encode_required_insert_count(section_ric, max_capacity)
    };
    FieldSectionPrefix {
        encoded_required_insert_count,
        base_is_negative: false,
        delta_base: 0,
    }
    .encode(buf);
}

/// Convert a planned [`Emission`] into a [`FieldLineInstruction`] and append its wire
/// encoding to `buf`. Pre-base dynamic references are translated from absolute to relative
/// indices using `base = section_ric` (so relative index is `section_ric - 1 - abs_idx`).
fn emit(emission: &Emission<'_, '_>, section_ric: u64, buf: &mut Vec<u8>) {
    let instruction = match emission {
        Emission::IndexedStatic(index) => FieldLineInstruction::IndexedStatic {
            index: usize::from(*index),
        },

        Emission::IndexedDynamicPreBase(abs_idx) => {
            debug_assert!(
                *abs_idx < section_ric,
                "IndexedDynamicPreBase: abs_idx {abs_idx} >= base {section_ric}"
            );
            FieldLineInstruction::IndexedDynamic {
                relative_index: usize::try_from(section_ric - 1 - abs_idx).unwrap_or(usize::MAX),
            }
        }

        Emission::LiteralStaticNameRef {
            name_index,
            value,
            never_indexed,
        } => FieldLineInstruction::LiteralStaticNameRef {
            name_index: usize::from(*name_index),
            value: value.reborrow(),
            never_indexed: *never_indexed,
        },

        Emission::LiteralDynamicNameRefPreBase {
            abs_idx,
            value,
            never_indexed,
        } => {
            debug_assert!(
                *abs_idx < section_ric,
                "LiteralDynamicNameRefPreBase: abs_idx {abs_idx} >= base {section_ric}"
            );
            FieldLineInstruction::LiteralDynamicNameRef {
                relative_index: usize::try_from(section_ric - 1 - *abs_idx).unwrap_or(usize::MAX),
                value: value.reborrow(),
                never_indexed: *never_indexed,
            }
        }

        Emission::LiteralLiteralName {
            name,
            value,
            never_indexed,
        } => FieldLineInstruction::LiteralLiteralName {
            name: name.reborrow(),
            value: value.reborrow(),
            never_indexed: *never_indexed,
        },
    };
    instruction.encode(buf);
}

/// Encode a Required Insert Count per RFC 9204 §4.5.1.1.
///
/// Returns 0 for a section that references no dynamic-table entries. Otherwise returns
/// `(ric mod (2 * max_entries)) + 1`, where `max_entries = max_capacity / 32`.
///
/// Callers must only pass `ric > 0` when `max_capacity >= 32`: a section with dynamic
/// references implies that the encoder has inserted at least one entry, which in turn
/// requires a non-zero `max_capacity`. A debug assertion enforces this.
pub(super) fn encode_required_insert_count(ric: u64, max_capacity: usize) -> usize {
    if ric == 0 {
        return 0;
    }

    let max_entries = (max_capacity / 32) as u64;

    debug_assert!(
        max_entries > 0,
        "encode_required_insert_count: ric>0 with max_capacity<32"
    );

    let full_range = 2 * max_entries;

    usize::try_from((ric % full_range) + 1)
        .expect("RIC fits in usize because max_capacity is a usize")
}
