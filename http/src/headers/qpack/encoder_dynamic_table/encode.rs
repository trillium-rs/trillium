//! Dynamic-table-aware QPACK field section encoder (RFC 9204 §4.5).
//!
//! Two-phase design:
//!
//! 1. **Plan phase** runs under the `TableState` mutex. It walks the field section, picks a
//!    strategy per field line (dynamic full match → static full match → static name ref → literal
//!    name), tracks the section's Required Insert Count and min-referenced `abs_idx`, enforces the
//!    blocked-streams budget at the first transition from non-blocking to blocking, and registers
//!    the outstanding section with the table — all before dropping the lock. Registration happens
//!    inside the critical section so the budget check and slot consumption are atomic.
//!
//! 2. **Emit phase** runs lock-free. It converts the planned [`Emission`] list into wire bytes
//!    using the §4.5 wire helpers.
//!
//! Supported strategies in this revision:
//!
//! 1. Dynamic full match (§4.5.2, T=0)
//! 2. Static full match (§4.5.2, T=1)
//! 3. Insert-then-reference (§3.2 insert + §4.5.2 T=0 ref in the same section)
//! 4. Static name reference (§4.5.4, T=1)
//! 5. Dynamic name reference (§4.5.4, T=0) — only when no static name match exists
//! 6. Literal with literal name (§4.5.6)
//!
//! The ordering is a deliberately simple starting point, not a tuned policy. Eventually the
//! strategy selection should live in a dedicated policy module that can consult table state,
//! per-peer statistics, and field-section hints (see qpack-n-bit-gap memory and the planned
//! litespeed-style policy work). Until then, the chain is static and the sensitive-header
//! skip list below is the only policy knob.

use super::{EncoderDynamicTable, SectionRefs, state::TableState};
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
    },

    /// §4.5.4: Literal Field Line with Name Reference, against the dynamic table (T=0),
    /// pre-base. Stored as the absolute table index; converted to the relative index at emit
    /// time.
    LiteralDynamicNameRefPreBase {
        abs_idx: u64,
        value: FieldLineValue<'lines>,
    },

    /// §4.5.6: Literal Field Line with Literal Name.
    LiteralLiteralName {
        name: &'lines QpackEntryName<'names>,
        value: FieldLineValue<'lines>,
    },
}

/// Output of the plan phase.
struct Plan<'lines, 'names> {
    emissions: Vec<Emission<'lines, 'names>>,
    section_ric: u64,
    min_ref_abs_idx: Option<u64>,
}

/// Planning-phase state. Holds a mutable reference to `TableState` and accumulates decisions
/// as the field section is walked. The plan phase runs entirely under the `TableState` lock.
/// Mutable access is required because the insert-then-reference strategy inserts new
/// dynamic-table entries during planning.
struct Planner<'state, 'lines, 'names> {
    state: &'state mut TableState,
    stream_id: u64,
    emissions: Vec<Emission<'lines, 'names>>,
    section_ric: u64,
    min_ref_abs_idx: Option<u64>,
    /// Set when this encode has already consumed its blocked-streams slot (the first
    /// blocking reference succeeded the budget check). Subsequent blocking references
    /// within the same section inherit the commitment and skip the re-check.
    committed_to_blocking: bool,
    /// Set when the plan phase has enqueued at least one encoder-stream insert. The caller
    /// uses this flag after dropping the lock to notify the encoder-stream writer.
    made_inserts: bool,
}

impl<'state, 'lines, 'names> Planner<'state, 'lines, 'names> {
    fn new(state: &'state mut TableState, stream_id: u64) -> Self {
        Self {
            state,
            stream_id,
            emissions: Vec::new(),
            section_ric: 0,
            min_ref_abs_idx: None,
            committed_to_blocking: false,
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

    fn plan_header_line(
        &mut self,
        name: &'lines QpackEntryName<'names>,
        value: FieldLineValue<'lines>,
    ) {
        if let Some(e) = self.try_dynamic_full_match(name, &value) {
            self.emissions.push(e);
            return;
        }

        let static_name_hint = match static_table_lookup(name, Some(&*value)) {
            StaticLookup::FullMatch(i) => {
                self.emissions.push(Emission::IndexedStatic(i));
                return;
            }
            StaticLookup::NameMatch(i) => Some(i),
            StaticLookup::NoMatch => None,
        };

        if let Some(e) = self.try_insert_then_reference(name, &value) {
            self.emissions.push(e);
            return;
        }

        let emission = if let Some(name_index) = static_name_hint {
            Some(Emission::LiteralStaticNameRef {
                name_index,
                value: value.clone(),
            })
        } else {
            self.try_dynamic_name_ref(name, value.clone())
        }
        .unwrap_or(Emission::LiteralLiteralName { name, value });

        self.emissions.push(emission);
    }

    /// Try to emit `(name, value)` as a dynamic-table full match. Returns `None` if no such
    /// entry exists, or if the match would exceed the blocked-streams budget.
    fn try_dynamic_full_match(
        &mut self,
        name: &'lines QpackEntryName<'names>,
        value: &FieldLineValue<'lines>,
    ) -> Option<Emission<'lines, 'names>> {
        let abs_idx = self
            .state
            .by_name
            .get(name)
            .and_then(|idx| idx.by_value.get(value.as_bytes()).copied())?;

        if !self.authorize_dynamic_ref(abs_idx) {
            return None;
        }

        self.record_ref(abs_idx);
        Some(Emission::IndexedDynamicPreBase(abs_idx))
    }

    /// Try to insert `(name, value)` into the dynamic table and emit an indexed reference to
    /// the freshly-inserted entry in the same section.
    ///
    /// Returns `None` when: the name is on the sensitive-header skip list, the
    /// blocked-streams budget is exhausted, or `TableState::insert` fails (entry wouldn't
    /// fit under `capacity`, or eviction under the combined pin floor + in-progress
    /// `min_ref_abs_idx` cannot free enough room).
    ///
    /// `insert` picks the §3.2 wire format internally based on the table's contents at the
    /// moment of insert.
    fn try_insert_then_reference(
        &mut self,
        entry_name: &'lines QpackEntryName<'names>,
        value_bytes: &FieldLineValue<'lines>,
    ) -> Option<Emission<'lines, 'names>> {
        if is_sensitive_header_name(entry_name) {
            return None;
        }

        // An insert-then-reference produces an abs_idx >= insert_count > known_received_count,
        // so the reference is always blocking. Consult the same authorization path as
        // dynamic refs — covers the committed_to_blocking dedup.
        if !self.committed_to_blocking
            && !self.state.is_stream_blocking(self.stream_id)
            && self.state.currently_blocked_streams() >= self.state.max_blocked_streams
        {
            return None;
        }

        let abs_idx = self
            .state
            .insert(
                entry_name.reborrow(),
                value_bytes.reborrow(),
                self.min_ref_abs_idx,
            )
            .ok()?;

        self.made_inserts = true;
        self.committed_to_blocking = true;
        self.record_ref(abs_idx);
        Some(Emission::IndexedDynamicPreBase(abs_idx))
    }

    /// Try to emit `(name, value)` as a literal field line with a dynamic-table name
    /// reference (§4.5.4, T=0). Used when the dynamic table has any entry matching `name` but
    /// none matching the full `(name, value)` pair, and the static table offers no name match.
    /// Returns `None` if no dynamic name entry exists, or if the reference would exceed the
    /// blocked-streams budget.
    fn try_dynamic_name_ref(
        &mut self,
        name: &QpackEntryName<'_>,
        value: FieldLineValue<'lines>,
    ) -> Option<Emission<'lines, 'names>> {
        let abs_idx = self.state.by_name.get(name).map(|idx| idx.latest_any)?;

        if !self.authorize_dynamic_ref(abs_idx) {
            return None;
        }

        self.record_ref(abs_idx);
        Some(Emission::LiteralDynamicNameRefPreBase { abs_idx, value })
    }

    /// Decide whether the current section may reference the entry at `abs_idx`. Returns
    /// false if the reference would push the section over the blocked-streams budget.
    fn authorize_dynamic_ref(&mut self, abs_idx: u64) -> bool {
        // Non-blocking: the peer has already processed this entry.
        if abs_idx < self.state.known_received_count {
            return true;
        }
        // Already committed: an earlier reference in this encode consumed the slot.
        if self.committed_to_blocking {
            return true;
        }
        // First blocking reference on this stream in this encode: check the budget.
        if self.state.is_stream_blocking(self.stream_id)
            || self.state.currently_blocked_streams() < self.state.max_blocked_streams
        {
            self.committed_to_blocking = true;
            return true;
        }
        false
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
/// TODO(n-bit): `never_indexed` is hardcoded to `false`. Preserving the N bit for
/// trillium-proxy reverse-proxy correctness requires threading it through `FieldLine` →
/// `FieldSection` → `Emission` first. See `qpack-n-bit-gap` memory.
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

        Emission::LiteralStaticNameRef { name_index, value } => {
            FieldLineInstruction::LiteralStaticNameRef {
                name_index: usize::from(*name_index),
                value: value.reborrow(),
                never_indexed: false,
            }
        }

        Emission::LiteralDynamicNameRefPreBase { abs_idx, value } => {
            debug_assert!(
                *abs_idx < section_ric,
                "LiteralDynamicNameRefPreBase: abs_idx {abs_idx} >= base {section_ric}"
            );
            FieldLineInstruction::LiteralDynamicNameRef {
                relative_index: usize::try_from(section_ric - 1 - *abs_idx).unwrap_or(usize::MAX),
                value: value.reborrow(),
                never_indexed: false,
            }
        }

        Emission::LiteralLiteralName { name, value } => FieldLineInstruction::LiteralLiteralName {
            name: name.reborrow(),
            value: value.reborrow(),
            never_indexed: false,
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
