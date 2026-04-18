//! Dynamic-table-aware QPACK field section encoder (RFC 9204 §4.5).
//!
//! Two-phase design:
//!
//! 1. **Plan phase** runs under the `TableState` mutex. It walks the field section and runs
//!    the per-line decide-then-commit pipeline (lookup → [`select_program`] →
//!    `execute_program`), tracks the section's Required Insert Count and min-referenced
//!    `abs_idx`, enforces the blocked-streams budget at the first transition from
//!    non-blocking to blocking, and registers the outstanding section with the table — all
//!    before dropping the lock. Registration happens inside the critical section so the
//!    budget check and slot consumption are atomic.
//!
//! 2. **Emit phase** runs lock-free. It converts the planned [`Emission`] list into wire bytes
//!    using the §4.5 wire helpers.
//!
//! ## Per-line pipeline
//!
//! Each field line runs through three stages designed to keep selection pure and side
//! effects narrow:
//!
//! - **Lookup** — `Planner::compute_match_state` reads static and dynamic tables; produces a
//!   [`MatchState`]. No mutation, no budget logic.
//! - **Select** — [`select_program`] is a pure function over `(MatchState, allow_indexing,
//!   BudgetCtx, never_indexed)` that returns an [`EncoderProgram`]: which encoder-stream
//!   action, which dynamic-table action, and which header-block emission to perform.
//! - **Execute** — `Planner::execute_program` applies the table mutation (calling
//!   [`TableState::insert`](super::state::TableState::insert) which picks the §3.2 wire
//!   format internally), records the dynamic ref in the section's RIC/min-floor accounting,
//!   commits to the blocked-streams slot if needed, and returns the [`Emission`].
//!
//! Strategies emitted today (phase 1; warming-insert and policy-driven Duplicate land in
//! phase 2):
//!
//! 1. Dynamic full match (§4.5.2, T=0)
//! 2. Static full match (§4.5.2, T=1)
//! 3. Insert-then-reference (§3.2 insert + §4.5.2 T=0 ref in the same section)
//! 4. Static name reference (§4.5.4, T=1)
//! 5. Dynamic name reference (§4.5.4, T=0) — only when no static name match exists
//! 6. Literal with literal name (§4.5.6)
//!
//! The ordering is a deliberately simple starting point, not a tuned policy. The sensitive-
//! header skip list below is the only indexing veto today; the planned `IndexingPolicy`
//! trait (phase 3) will replace the hardcoded `should_index = true` seam in
//! `plan_header_line`.

use super::{
    EncoderDynamicTable, SectionRefs,
    policy::{
        BudgetCtx, DynRef, EncoderProgram, HeaderBlockAction, MatchState, TableAction,
        select_program,
    },
    state::TableState,
};
use crate::{
    KnownHeaderName,
    headers::qpack::{
        FieldLineValue, FieldSection,
        entry_name::QpackEntryName,
        instruction::field_section::{FieldLineInstruction, FieldSectionPrefix},
        static_table::{StaticLookup, static_table_lookup},
    },
};

/// Header names that must never be inserted into the QPACK dynamic table.
///
/// This is the current stand-in for propagating the RFC 9204 §4.5.4 N ("Never Indexed") bit
/// through trillium-proxy. Values under these names are emitted as literals without being
/// indexed, so a CRIME-style length side-channel against a shared dynamic table cannot learn
/// them. See `qpack-n-bit-gap` memory for the full N-bit story and the plan for a real fix.
///
/// Lowercase ASCII byte strings. HTTP/3 header names are required to be lowercase
/// (RFC 9114 §4.1.1), so no case folding is needed on lookup.
fn is_sensitive_header_name(name: &QpackEntryName) -> bool {
    matches!(
        name,
        QpackEntryName::Known(
            KnownHeaderName::Authorization
                | KnownHeaderName::Cookie
                | KnownHeaderName::SetCookie
                | KnownHeaderName::ProxyAuthorization
                | KnownHeaderName::AuthenticationInfo
        )
    )
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
        field_lines: &[(QpackEntryName<'_>, FieldLineValue<'_>)],
        buf: &mut Vec<u8>,
        stream_id: u64,
    ) {
        let plan;
        let max_capacity;
        let made_inserts;
        {
            let mut state = self.state.lock().unwrap();
            max_capacity = state.max_capacity;
            let mut planner = Planner::new(&mut state, stream_id);
            for (name, value) in field_lines {
                planner.plan_header_line(name, value.reborrow());
            }
            made_inserts = planner.made_inserts;
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

        emit_section_prefix(plan.section_ric, max_capacity, buf);
        for emission in &plan.emissions {
            emit(emission, plan.section_ric, buf);
        }
    }
}

/// A single field-line encoding decision produced by the plan phase.
///
/// Borrows name/value slices from the caller's `FieldSection` — the plan lives only for the
/// duration of one `encode()` call, so the lifetime is always trivially satisfied.
///
/// The literal variants carry a `never_indexed` flag (RFC 9204 §4.5.4 N bit). Hardcoded
/// `false` today because the source signal is not yet plumbed through `FieldLine` — see
/// `qpack-n-bit-gap` memory.
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
        name: &'lines QpackEntryName<'names>,
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
/// decisions as the field section is walked. The plan phase runs entirely under the
/// `TableState` lock. Mutable access is required because some programs insert new
/// dynamic-table entries during planning.
struct Planner<'state, 'lines, 'names> {
    state: &'state mut TableState,
    emissions: Vec<Emission<'lines, 'names>>,
    section_ric: u64,
    min_ref_abs_idx: Option<u64>,
    /// Set when this encode has already consumed its blocked-streams slot (the first
    /// blocking reference succeeded the budget check). Subsequent blocking references
    /// within the same section inherit the commitment and skip the re-check.
    committed_to_blocking: bool,
    /// Cached at planner construction: was this stream already blocking when we entered
    /// the section? Stable while we hold the `TableState` lock; lets `BudgetCtx` skip a
    /// repeated `is_stream_blocking` query per line.
    stream_already_blocking: bool,
    /// Set when the plan phase has enqueued at least one encoder-stream insert. The caller
    /// uses this flag after dropping the lock to notify the encoder-stream writer.
    made_inserts: bool,
}

impl<'state, 'lines, 'names> Planner<'state, 'lines, 'names> {
    fn new(state: &'state mut TableState, stream_id: u64) -> Self {
        let stream_already_blocking = state.is_stream_blocking(stream_id);
        Self {
            state,
            emissions: Vec::new(),
            section_ric: 0,
            min_ref_abs_idx: None,
            committed_to_blocking: false,
            stream_already_blocking,
            made_inserts: false,
        }
    }

    fn finish(self) -> Plan<'lines, 'names> {
        Plan {
            emissions: self.emissions,
            section_ric: self.section_ric,
            min_ref_abs_idx: self.min_ref_abs_idx,
        }
    }

    /// Plan a single field line through the decide-then-commit pipeline:
    /// `compute_match_state` (read) → [`select_program`] (pure decision) → `execute_program`
    /// (side effects + emission).
    fn plan_header_line(
        &mut self,
        name: &'lines QpackEntryName<'names>,
        value: FieldLineValue<'lines>,
    ) {
        // PHASE-3-SEAM: replace this with `policy.should_index(...)` once `IndexingPolicy`
        // lands. The sensitive-header skip is a stand-in for the eventual N-bit signal
        // (see `qpack-n-bit-gap` memory).
        let allow_indexing = !is_sensitive_header_name(name);
        // Source for `never_indexed` is hardcoded `false` until `FieldLine` carries the bit
        // end-to-end. Threading it through `EncoderProgram` and `Emission` now keeps the
        // structural change minimal when the source lands.
        let never_indexed = false;

        let emission = self.plan_with_indexing(name, value, allow_indexing, never_indexed);
        self.emissions.push(emission);
    }

    /// Inner planning loop. Selects and executes a program; on insert failure, re-selects
    /// with `allow_indexing = false` and tries again.
    ///
    /// `TableState::insert` can fail on capacity/eviction in ways `select_program` cannot
    /// pre-check from `MatchState` alone (entry size, pin floor, in-progress
    /// `min_ref_abs_idx`). The retry path mirrors the old `try_*` chain's `.ok()?`
    /// fallthrough — and recomputes `MatchState`, since a partial eviction inside `insert`
    /// may have changed which dynamic entries are live.
    fn plan_with_indexing(
        &mut self,
        name: &'lines QpackEntryName<'names>,
        value: FieldLineValue<'lines>,
        allow_indexing: bool,
        never_indexed: bool,
    ) -> Emission<'lines, 'names> {
        let match_state = self.compute_match_state(name, &value);
        let budget = self.budget_ctx();
        let program = select_program(&match_state, allow_indexing, &budget, never_indexed);

        if !matches!(program.table, TableAction::InsertNew) {
            return self.execute_program(program, name, value, None);
        }

        if let Ok(new_abs_idx) =
            self.state
                .insert(name.reborrow(), value.reborrow(), self.min_ref_abs_idx)
        {
            self.made_inserts = true;
            self.execute_program(program, name, value, Some(new_abs_idx))
        } else {
            debug_assert!(
                allow_indexing,
                "select_program only emits InsertNew when allow_indexing is true"
            );
            self.plan_with_indexing(name, value, false, never_indexed)
        }
    }

    /// Read the static and dynamic tables for `(name, value)` and report what matches. Pure
    /// — no mutation, no budget logic.
    fn compute_match_state(
        &self,
        name: &QpackEntryName<'_>,
        value: &FieldLineValue<'_>,
    ) -> MatchState {
        let dyn_name_idx = self.state.by_name.get(name);
        let dynamic_full = dyn_name_idx.and_then(|idx| idx.by_value.get(value.as_bytes()).copied());
        let dynamic_name = dyn_name_idx.map(|idx| idx.latest_any);

        let (static_full, static_name) = match static_table_lookup(name, Some(value.as_bytes())) {
            StaticLookup::FullMatch(i) => (Some(i), Some(i)),
            StaticLookup::NameMatch(i) => (None, Some(i)),
            StaticLookup::NoMatch => (None, None),
        };

        MatchState {
            dynamic_full,
            static_full,
            dynamic_name,
            static_name,
        }
    }

    /// Snapshot the blocked-streams budget for the next [`select_program`] call.
    fn budget_ctx(&self) -> BudgetCtx {
        BudgetCtx {
            krc: self.state.known_received_count,
            committed_to_blocking: self.committed_to_blocking,
            stream_already_blocking: self.stream_already_blocking,
            blocked_slot_available: self.state.currently_blocked_streams()
                < self.state.max_blocked_streams,
        }
    }

    /// Build the [`Emission`] for `program`, given any `new_abs_idx` produced by a preceding
    /// `TableAction::InsertNew` performed by [`plan_with_indexing`]. Records the dynamic ref
    /// in the section's RIC and min-floor accounting and commits to a blocked-streams slot
    /// when the ref is the first blocking one in this section.
    fn execute_program(
        &mut self,
        program: EncoderProgram,
        name: &'lines QpackEntryName<'names>,
        value: FieldLineValue<'lines>,
        new_abs_idx: Option<u64>,
    ) -> Emission<'lines, 'names> {
        debug_assert_eq!(
            new_abs_idx.is_some(),
            matches!(program.table, TableAction::InsertNew),
            "new_abs_idx presence must match TableAction::InsertNew"
        );

        match program.header_block {
            HeaderBlockAction::IndexedStatic(i) => Emission::IndexedStatic(i),
            HeaderBlockAction::IndexedDynamic(dyn_ref) => {
                let abs_idx = self.resolve_and_record_dyn_ref(dyn_ref, new_abs_idx);
                Emission::IndexedDynamicPreBase(abs_idx)
            }
            HeaderBlockAction::LiteralStaticNameRef { name_index } => {
                Emission::LiteralStaticNameRef {
                    name_index,
                    value,
                    never_indexed: program.never_indexed,
                }
            }
            HeaderBlockAction::LiteralDynamicNameRef { dyn_ref } => {
                let abs_idx = self.resolve_and_record_dyn_ref(dyn_ref, new_abs_idx);
                Emission::LiteralDynamicNameRefPreBase {
                    abs_idx,
                    value,
                    never_indexed: program.never_indexed,
                }
            }
            HeaderBlockAction::LiteralLiteralName => Emission::LiteralLiteralName {
                name,
                value,
                never_indexed: program.never_indexed,
            },
        }
    }

    /// Convert a [`DynRef`] into its absolute index, then update the section's RIC,
    /// min-floor, and blocked-slot commitment to account for the reference.
    fn resolve_and_record_dyn_ref(
        &mut self,
        dyn_ref: DynRef,
        new_abs_idx: Option<u64>,
    ) -> u64 {
        let abs_idx = match dyn_ref {
            DynRef::Existing(i) => i,
            DynRef::NewlyInserted => new_abs_idx
                .expect("NewlyInserted requires a preceding TableAction::InsertNew"),
        };
        if abs_idx >= self.state.known_received_count
            && !self.committed_to_blocking
            && !self.stream_already_blocking
        {
            self.committed_to_blocking = true;
        }
        self.record_ref(abs_idx);
        abs_idx
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
///
/// `never_indexed` is now carried per-emission. Phase 1 still sources it as `false` from
/// `Planner::plan_header_line` because `FieldLine` does not yet thread the bit through —
/// see `qpack-n-bit-gap` memory.
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
