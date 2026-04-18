//! Pure decision layer for the per-line encoder pipeline.
//!
//! The policy module owns the *what* of QPACK encoding: given what an entry matches in the
//! tables and how much budget is available, what should the encoder do? It deliberately
//! holds no references to the dynamic table, performs no I/O, and never mutates anything.
//! The mechanism — appending entries, queuing encoder-stream bytes, recording references —
//! lives in [`super::encode`].
//!
//! ## Pipeline
//!
//! For each field line, the encode-phase planner runs:
//!
//! ```text
//! compute_match_state(state, name, value) → MatchState
//!     └── select_program(MatchState, allow_indexing, BudgetCtx, never_indexed) → EncoderProgram
//!             └── execute_program(EncoderProgram, name, value)                  → Emission
//! ```
//!
//! [`select_program`] is a pure function. It is the seat of the encoder's compression
//! policy and the home of phase-3+ extensions: the planned `IndexingPolicy` trait
//! (mnemonic predictor, EMAs), draining detection, inflation guards, and per-encode insert
//! budgets all hang off this function.
//!
//! ## Action triple
//!
//! An [`EncoderProgram`] is split into three orthogonal axes — `enc_stream`, `table`, and
//! `header_block` — even though phase 1 only emits two `(enc_stream, table)` pairs. The
//! split anticipates phase 2's warming insert (`Insert` + `InsertNew` + `LiteralLiteralName`,
//! i.e. an insert without an in-section reference) and policy-driven `Duplicate`. Carrying
//! the structure now keeps phase-2 code arms additive rather than restructuring.

/// What an entry at `(name, value)` matches in the static and dynamic tables. Pure read of
/// table state — `Planner::compute_match_state` produces it without consulting the budget
/// or any per-section accounting. Input to [`select_program`].
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct MatchState {
    /// Absolute index of a dynamic-table entry that matches both name and value, if any.
    pub(super) dynamic_full: Option<u64>,
    /// Static-table index of an entry that matches both name and value, if any.
    pub(super) static_full: Option<u8>,
    /// Absolute index of the latest dynamic-table entry whose name matches, if any. May
    /// equal `dynamic_full` when a full match exists.
    pub(super) dynamic_name: Option<u64>,
    /// Static-table index of an entry whose name matches, if any. May equal `static_full`
    /// when a full match exists.
    pub(super) static_name: Option<u8>,
}

/// Encoder-stream side effect for one field line. Phase 1 only emits `None` or `Insert`;
/// phase 2 will add `Duplicate(abs_idx)` for policy-driven Duplicate.
#[derive(Clone, Copy, Debug)]
pub(super) enum EncStreamAction {
    /// No encoder-stream bytes for this line.
    None,
    /// Append a §3.2 Insert to the encoder stream. The wire variant (Insert With Name
    /// Reference T=1/T=0, Insert With Literal Name, Duplicate) is picked inside
    /// [`TableState::insert`](super::state::TableState::insert) from the table's contents
    /// at insert time.
    Insert,
}

/// Dynamic-table side effect for one field line. Always paired with the matching
/// [`EncStreamAction`] — `Insert` ↔ `InsertNew`. Phase 2 will add `Duplicate(abs_idx)`.
#[derive(Clone, Copy, Debug)]
pub(super) enum TableAction {
    /// Table state is unchanged for this line.
    None,
    /// Insert a new entry; the resulting absolute index is fed back to the executor so it
    /// can resolve any `DynRef::NewlyInserted` reference in the header block.
    InsertNew,
}

/// How a header-block dynamic reference identifies its target entry. `NewlyInserted` is
/// resolved by the executor from the `abs_idx` returned by the table-action insert; using
/// it without a preceding `TableAction::InsertNew` is a planner bug.
#[derive(Clone, Copy, Debug)]
pub(super) enum DynRef {
    /// An entry already in the dynamic table, identified by absolute index.
    Existing(u64),
    /// The entry produced by this program's `TableAction::InsertNew`.
    NewlyInserted,
}

/// Header-block emission for one field line. Carries enough information to build an
/// `Emission` but no borrows of the field name or value — those stay in the planner's
/// scope and are wired into the `Emission` by the executor. Keeping this lifetime-free
/// lets [`select_program`] be a fully pure function.
#[derive(Clone, Copy, Debug)]
pub(super) enum HeaderBlockAction {
    /// §4.5.2 Indexed Field Line, T=1.
    IndexedStatic(u8),
    /// §4.5.2 Indexed Field Line, T=0, pre-base.
    IndexedDynamic(DynRef),
    /// §4.5.4 Literal Field Line With Name Reference, T=1.
    LiteralStaticNameRef { name_index: u8 },
    /// §4.5.4 Literal Field Line With Name Reference, T=0, pre-base.
    LiteralDynamicNameRef { dyn_ref: DynRef },
    /// §4.5.6 Literal Field Line With Literal Name.
    LiteralLiteralName,
}

/// Per-line plan: encoder-stream action × table action × header-block action × N bit. The
/// product of [`select_program`]; consumed by the executor in [`super::encode`].
///
/// Today only two `(enc_stream, table)` pairs are valid: `(None, None)` and
/// `(Insert, InsertNew)`. Phase 2 adds `(Insert, InsertNew)` paired with
/// `LiteralLiteralName` (warming insert) and a `Duplicate(abs_idx)` pair.
#[derive(Clone, Copy, Debug)]
pub(super) struct EncoderProgram {
    pub(super) enc_stream: EncStreamAction,
    pub(super) table: TableAction,
    pub(super) header_block: HeaderBlockAction,
    /// RFC 9204 §4.5.4 N bit. Hardcoded `false` today; will become a per-line input once
    /// the source signal is plumbed through `FieldLine` (see `qpack-n-bit-gap` memory).
    pub(super) never_indexed: bool,
}

/// Snapshot of the blocked-streams budget at the moment [`select_program`] is consulted.
/// Stable for the duration of one `select_program` call: built by `Planner::budget_ctx`
/// from the `TableState` lock + the planner's own per-section flags. Pure input —
/// `select_program` never mutates it.
#[derive(Clone, Copy, Debug)]
pub(super) struct BudgetCtx {
    /// `TableState::known_received_count`. An entry with `abs_idx < krc` is non-blocking.
    pub(super) krc: u64,
    /// This encode has already consumed its blocked-streams slot via an earlier reference
    /// in the same section. Subsequent blocking references are free.
    pub(super) committed_to_blocking: bool,
    /// The stream this encode targets is already blocking at section start (some prior
    /// section is still pending an ack). Blocking references on this section don't take a
    /// new slot.
    pub(super) stream_already_blocking: bool,
    /// `currently_blocked_streams() < max_blocked_streams` — a fresh slot is available.
    pub(super) blocked_slot_available: bool,
}

impl BudgetCtx {
    /// True iff a header-block reference to `abs_idx` is allowed under the blocked-streams
    /// budget. Never-blocking refs (`abs_idx < krc`) are always allowed.
    fn allows_ref(&self, abs_idx: u64) -> bool {
        if abs_idx < self.krc {
            return true;
        }
        self.committed_to_blocking || self.stream_already_blocking || self.blocked_slot_available
    }

    /// True iff this section is permitted to take a fresh blocking slot. An insert-then-
    /// reference always produces an `abs_idx >= insert_count > krc`, so the new ref is
    /// unconditionally blocking.
    fn can_take_blocking_slot(&self) -> bool {
        self.committed_to_blocking || self.stream_already_blocking || self.blocked_slot_available
    }
}

/// Pure decision: pick the [`EncoderProgram`] for one field line given what the entry
/// matches, whether indexing is allowed, and the current budget snapshot.
///
/// Phase 1 strategy chain (hardcoded; the planned `IndexingPolicy` trait will replace the
/// caller's `allow_indexing` decision in phase 3):
///
/// 1. Dynamic full match if budget allows the ref.
/// 2. Static full match.
/// 3. Insert-then-reference if `allow_indexing` and a blocking slot is available.
/// 4. Static name ref (preferred over dynamic name ref).
/// 5. Dynamic name ref (only when no static name match and budget allows).
/// 6. Literal name + literal value.
pub(super) fn select_program(
    match_state: &MatchState,
    allow_indexing: bool,
    budget: &BudgetCtx,
    never_indexed: bool,
) -> EncoderProgram {
    let mk = |header_block| EncoderProgram {
        enc_stream: EncStreamAction::None,
        table: TableAction::None,
        header_block,
        never_indexed,
    };

    if let Some(abs_idx) = match_state.dynamic_full
        && budget.allows_ref(abs_idx)
    {
        return mk(HeaderBlockAction::IndexedDynamic(DynRef::Existing(abs_idx)));
    }

    if let Some(idx) = match_state.static_full {
        return mk(HeaderBlockAction::IndexedStatic(idx));
    }

    if allow_indexing && budget.can_take_blocking_slot() {
        return EncoderProgram {
            enc_stream: EncStreamAction::Insert,
            table: TableAction::InsertNew,
            header_block: HeaderBlockAction::IndexedDynamic(DynRef::NewlyInserted),
            never_indexed,
        };
    }

    if let Some(name_index) = match_state.static_name {
        return mk(HeaderBlockAction::LiteralStaticNameRef { name_index });
    }

    if let Some(abs_idx) = match_state.dynamic_name
        && budget.allows_ref(abs_idx)
    {
        return mk(HeaderBlockAction::LiteralDynamicNameRef {
            dyn_ref: DynRef::Existing(abs_idx),
        });
    }

    mk(HeaderBlockAction::LiteralLiteralName)
}
