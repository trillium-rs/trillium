//! Internal storage and mutation logic for the encoder dynamic table.
//!
//! [`TableState`] holds the entries, capacity, KRC, outstanding sections, reverse-index, and
//! pending op queue. Single mutating entry point: [`TableState::insert`] picks the smallest
//! §3.2 wire format from the table's current contents — including a Duplicate fast-path when
//! `(name, value)` already matches a live entry.
//!
//! Wire-format selection lives in `insert`, not in callers. Policy code says "insert this
//! header" — the sub-variant choice (duplicate / literal name / static name ref / dynamic
//! name ref) is a deterministic function of the table's contents at insert time.
//!
//! All mutations go through `insert`, `set_capacity`, or the ack/cancel/increment helpers
//! in the parent module. This file does no I/O — wire bytes are pushed onto `pending_ops`
//! for the writer task to drain.

use super::predictor::MnemonicPredictor;
#[cfg(test)]
use super::strategy_counters::StrategyCounters;

/// Saturating `usize` → `u32` conversion. Wire byte sizes and header lengths never
/// meaningfully exceed `u32::MAX` in practice; clamping at the boundary keeps the
/// inflation counter arithmetic honest without adding a panic on pathological inputs.
pub(super) fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}
use crate::{
    h3::{H3Error, H3ErrorCode},
    headers::qpack::{
        FieldLineValue,
        entry_name::QpackEntryName,
        instruction::encoder::{
            encode_duplicate, encode_insert_with_literal_name, encode_insert_with_name_ref,
            encode_set_capacity,
        },
        static_table::first_match,
    },
};
use std::{
    borrow::Cow,
    collections::{HashMap, VecDeque},
};

#[derive(Debug)]
pub(super) struct TableState {
    /// Entries in insertion order, newest first. `entries[0]` has absolute index
    /// `insert_count - 1`; `entries[i]` has absolute index `insert_count - 1 - i`.
    pub(super) entries: VecDeque<Entry>,
    /// Upper bound on `capacity`. Typically `min(our_configured_limit, peer_advertised_max)`.
    /// A `SetCapacity` enqueue exceeding this is a bug.
    pub(super) max_capacity: usize,
    /// Current working capacity (bytes). Changed by enqueueing a Set Dynamic Table Capacity
    /// instruction; always ≤ `max_capacity`.
    pub(super) capacity: usize,
    /// Sum of `entry.size` for all live entries.
    pub(super) current_size: usize,
    /// Total entries ever inserted (monotonically increasing). Equals one past the absolute
    /// index of the most-recently inserted entry.
    pub(super) insert_count: u64,
    /// Largest `insert_count` value the peer's decoder is known to have processed. Advanced
    /// by Section Acknowledgement and Insert Count Increment instructions. Entries with
    /// absolute index `< known_received_count` are safely referenced by header blocks
    /// without blocking the peer's decoder.
    pub(super) known_received_count: u64,
    /// Wire-encoded encoder-stream instructions waiting to be written. Each entry is one
    /// full instruction. Drained in FIFO order; the writer must write them in order.
    pub(super) pending_ops: VecDeque<Vec<u8>>,
    /// Per-stream outstanding header sections. Each section records the entries it pinned.
    /// Drained by Section Acknowledgement (oldest first) and Stream Cancellation (all).
    pub(super) outstanding_sections: HashMap<u64, VecDeque<SectionRefs>>,
    /// Set when the encoder or decoder stream fails; wakes the writer task so it can exit.
    pub(super) failed: Option<H3ErrorCode>,
    /// Maximum number of streams that may be simultaneously blocked on pending inserts,
    /// from the peer's `SETTINGS_QPACK_BLOCKED_STREAMS`.
    pub(super) max_blocked_streams: usize,
    /// Reverse index for encode-path lookups. Outer map keyed by entry name; each
    /// [`NameIndex`] holds a per-value map (for full-match lookups) and the latest `abs_idx`
    /// across all live entries with this name (for name-only lookups).
    pub(super) by_name: HashMap<QpackEntryName<'static>, NameIndex>,
    /// Whether the planner should consult [`predictor`](Self::predictor) when deciding
    /// `allow_indexing`. When `false`, every non-sensitive header is considered indexable
    /// (phase-2 eager behavior). Sourced from `HttpConfig::h3_qpack_mnemonic_indexing`.
    pub(super) mnemonic_indexing: bool,
    /// Mnemonic predictor state. Always present so the struct has a single shape; the
    /// [`mnemonic_indexing`](Self::mnemonic_indexing) flag gates whether the planner
    /// actually consults it.
    pub(super) predictor: MnemonicPredictor,
    /// Running sum of raw `name.len() + value.len()` across all header lines encoded on
    /// this connection. Paired with [`bytes_out`](Self::bytes_out) to drive the phase-5
    /// inflation guard. Periodically rescaled by
    /// [`rescale_compression_counters`](Self::rescale_compression_counters) to prevent
    /// `u32` overflow; rescaling preserves the ratio.
    pub(super) bytes_in: u32,
    /// Running sum of wire bytes (encoder stream + header block) emitted for this
    /// connection.
    pub(super) bytes_out: u32,
    /// Inflation guard threshold: when a planned Insert-paired-with-literal would project
    /// the running ratio `bytes_out / bytes_in` above this value, the line is re-planned
    /// with indexing disabled. `1.0` or higher disables the guard. Sourced from
    /// `HttpConfig::h3_qpack_inflation_ratio_max`.
    pub(super) inflation_ratio_max: f32,
    /// Development-time strategy counters. `Some` only when the corpus test opts in via
    /// [`EncoderDynamicTable::enable_strategy_counters`](super::EncoderDynamicTable::enable_strategy_counters).
    /// Always `None` in production (the field itself is `cfg(test)`-gated).
    #[cfg(test)]
    pub(super) strategy_counters: Option<StrategyCounters>,
}

#[derive(Debug, Default)]
pub(super) struct NameIndex {
    /// Per-value map of live `abs_idx` values. Values are raw bytes so the encode path can
    /// probe the map with `&[u8]` (e.g. `str::as_bytes`) without allocating a `HeaderValue`
    /// just to build the lookup key.
    pub(super) by_value: HashMap<Cow<'static, [u8]>, u64>,
    /// Latest `abs_idx` across all entries in `by_value`. Recomputed on eviction when the
    /// evicted entry was the latest; `by_value.values().max()` is cheap because the same
    /// name rarely has many simultaneous live values.
    pub(super) latest_any: u64,
}

#[derive(Debug, Clone)]
pub(super) struct Entry {
    pub(super) name: QpackEntryName<'static>,
    pub(super) value: Cow<'static, [u8]>,
    /// `name.len() + value.len() + 32` per RFC 9204 §3.2.1.
    pub(super) size: usize,
}

/// References held by a single outstanding header section. Used to pin entries against
/// eviction until the peer acknowledges the section.
#[derive(Debug, Clone, Copy)]
pub(in crate::headers) struct SectionRefs {
    /// Required Insert Count for this section (one past the highest absolute index
    /// referenced). Becomes the new `known_received_count` when this section is acked,
    /// if larger than the current value.
    pub(in crate::headers) required_insert_count: u64,
    /// Smallest absolute index referenced by this section, if any. Contributes to the
    /// eviction floor while this section is outstanding. `None` if the section referenced
    /// only static-table entries.
    pub(in crate::headers) min_ref_abs_idx: Option<u64>,
}

impl TableState {
    pub(super) fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            max_capacity: 0,
            capacity: 0,
            current_size: 0,
            insert_count: 0,
            known_received_count: 0,
            pending_ops: VecDeque::new(),
            outstanding_sections: HashMap::new(),
            failed: None,
            max_blocked_streams: 0,
            by_name: HashMap::new(),
            // Set by `EncoderDynamicTable::initialize_from_peer_settings` from
            // `HttpConfig::h3_qpack_mnemonic_indexing` / `h3_qpack_inflation_ratio_max`.
            mnemonic_indexing: false,
            predictor: MnemonicPredictor::new(),
            bytes_in: 0,
            bytes_out: 0,
            // `1.0` disables the guard; the real default is re-applied by
            // `initialize_from_peer_settings`.
            inflation_ratio_max: 1.0,
            #[cfg(test)]
            strategy_counters: None,
        }
    }

    /// Insert `(name, value)` into the table, smart-picking the §3.2 wire format.
    ///
    /// Selection order (smallest wire encoding first):
    /// 1. **Duplicate** (§3.2.4) if `(name, value)` already matches a live entry — refreshes that
    ///    entry to the head of the table without re-sending name or value bytes.
    /// 2. **Insert With Name Reference, T=1** if `name` has any static slot.
    /// 3. **Insert With Name Reference, T=0** if a live entry already has this `name`.
    /// 4. **Insert With Literal Name**.
    ///
    /// `extra_floor` is an additional eviction-floor `abs_idx` that must be preserved across
    /// any eviction performed by this insert (combined with the outstanding-sections pin
    /// floor and any variant-specific preserve floor). The encode-path planner uses this to
    /// hold an in-progress section's smallest referenced `abs_idx` alive across an
    /// insert-then-reference; everywhere else it's `None`.
    ///
    /// The Duplicate and dynamic-name-ref paths add the referenced entry's `abs_idx` to the
    /// eviction floor for the duration of `make_room_for`, so eviction can't drop the entry
    /// whose name (and possibly value) we're about to copy.
    ///
    /// Returns the absolute index of the freshly-inserted entry on success.
    ///
    /// # Errors
    ///
    /// Returns `H3ErrorCode::QpackEncoderStreamError` if the entry alone exceeds `capacity`
    /// or if eviction would require dropping a pinned entry (combined floor).
    pub(super) fn insert(
        &mut self,
        name: QpackEntryName<'_>,
        value: FieldLineValue<'_>,
        extra_floor: Option<u64>,
    ) -> Result<u64, H3Error> {
        if let Some(abs_idx) = self
            .by_name
            .get(&name)
            .and_then(|idx| idx.by_value.get(value.as_bytes()).copied())
        {
            return self.duplicate(abs_idx, extra_floor);
        }

        let entry_size = name.len() + value.len() + 32;

        let (wire, variant_floor) = if let Some(static_idx) = first_match(&name) {
            (
                encode_insert_with_name_ref(usize::from(static_idx), true, &value),
                None,
            )
        } else if let Some(name_abs_idx) = self.by_name.get(&name).map(|idx| idx.latest_any) {
            let relative_index = self.insert_count - 1 - name_abs_idx;
            let wire = encode_insert_with_name_ref(
                usize::try_from(relative_index).unwrap_or(usize::MAX),
                false,
                &value,
            );
            (wire, Some(name_abs_idx))
        } else {
            (
                encode_insert_with_literal_name(name.as_bytes(), &value),
                None,
            )
        };

        self.make_room_for(entry_size, combine_floor(variant_floor, extra_floor))?;
        // Eviction succeeded — only now allocate the owned form of the value.
        let value = value.into_static();
        Ok(self.insert_entry(name, value, entry_size, wire))
    }

    /// §3.2.4 Duplicate. Two callers:
    ///
    /// - [`insert`](Self::insert)'s smart-pick fast-path, when the caller's `(name, value)` already
    ///   matches a live entry. The source's stored name+value are cloned (cheap `Cow` clones for
    ///   the common `'static` case) rather than allocating fresh owned copies from the borrowed
    ///   inputs.
    /// - The encode-phase planner, when policy decides to refresh a specific live entry into a
    ///   fresh table position (e.g. phase 4's draining-refresh) regardless of the field line being
    ///   encoded.
    ///
    /// The source `abs_idx` is added to the eviction floor for the duration of
    /// `make_room_for` so it remains live for the post-eviction clone.
    pub(in crate::headers::qpack::encoder_dynamic_table) fn duplicate(
        &mut self,
        abs_idx: u64,
        extra_floor: Option<u64>,
    ) -> Result<u64, H3Error> {
        let entry_size = self
            .entry_at_abs(abs_idx)
            .expect("insert's by_value lookup guarantees abs_idx is live")
            .size;
        let relative_index = self.insert_count - 1 - abs_idx;
        let wire = encode_duplicate(usize::try_from(relative_index).unwrap_or(usize::MAX));

        self.make_room_for(entry_size, combine_floor(Some(abs_idx), extra_floor))?;
        // Preserve floor guarantees `abs_idx` is still live; clone its name+value now —
        // deferred past eviction so a `Cow::Owned` value isn't allocated on failure.
        let entry = self
            .entry_at_abs(abs_idx)
            .expect("preserved by make_room_for floor");
        let name = entry.name.clone();
        let value = entry.value.clone();
        Ok(self.insert_entry(name, value, entry_size, wire))
    }

    /// Set the working capacity and emit a Set Dynamic Table Capacity instruction
    /// (RFC 9204 §3.2.1, §4.3.1). Evicts oldest entries that no longer fit, respecting the
    /// outstanding-sections pin floor.
    ///
    /// # Errors
    ///
    /// Returns an error if `new_capacity > max_capacity` or if eviction would require
    /// dropping a pinned entry.
    pub(super) fn set_capacity(&mut self, new_capacity: usize) -> Result<(), H3Error> {
        if new_capacity > self.max_capacity {
            log::error!(
                "qpack encoder: set_capacity {} exceeds max_capacity {}",
                new_capacity,
                self.max_capacity,
            );
            return Err(H3ErrorCode::QpackEncoderStreamError.into());
        }
        self.evict_down_to(new_capacity)?;
        self.capacity = new_capacity;
        self.pending_ops
            .push_back(encode_set_capacity(new_capacity));
        Ok(())
    }

    /// Validate `entry_size` against `capacity` and evict oldest entries until a new entry
    /// of `entry_size` bytes will fit. Single eviction step for every insert variant.
    ///
    /// Respects both the outstanding-sections pin floor and an optional `extra_floor`.
    /// Returns `Err` without mutating if the entry does not fit under `capacity`, or if
    /// eviction would require dropping an entry below either floor.
    ///
    /// Callers must convert the value to its owned form (and clone any source) only after
    /// this call succeeds — the deferred-allocation contract — and then immediately call
    /// [`insert_entry`](Self::insert_entry) under the same lock.
    fn make_room_for(
        &mut self,
        entry_size: usize,
        extra_floor: Option<u64>,
    ) -> Result<(), H3Error> {
        if entry_size > self.capacity {
            return Err(H3ErrorCode::QpackEncoderStreamError.into());
        }
        let target = self.capacity - entry_size;
        let combined_floor = combine_floor(self.eviction_floor(), extra_floor);
        self.evict_down_to_with_floor(target, combined_floor)
    }

    /// Commit a new entry to the table: push it onto `entries`, update `current_size` and
    /// `insert_count`, update the reverse index, and enqueue the wire bytes for the writer.
    /// Callers must have already called [`make_room_for`](Self::make_room_for) under the
    /// same lock; this helper does no validation.
    fn insert_entry(
        &mut self,
        name: QpackEntryName<'_>,
        value: Cow<'static, [u8]>,
        entry_size: usize,
        wire: Vec<u8>,
    ) -> u64 {
        let name = name.into_owned();
        let abs_idx = self.insert_count;
        let name_index = self.by_name.entry(name.clone()).or_default();
        name_index.by_value.insert(value.clone(), abs_idx);
        name_index.latest_any = abs_idx;
        self.entries.push_front(Entry {
            name,
            value,
            size: entry_size,
        });
        self.current_size += entry_size;
        self.insert_count += 1;
        self.pending_ops.push_back(wire);
        log::trace!(
            "qpack encoder: inserted entry abs_idx={abs_idx} size={entry_size} current_size={} \
             insert_count={}",
            self.current_size,
            self.insert_count,
        );
        abs_idx
    }

    /// Phase-5 inflation guard: would indexing this `(name, value)` pair at the current
    /// running ratio push `bytes_out / bytes_in` above the configured threshold?
    ///
    /// Matches ls-qpack's projection (`lsqpack.c:1945-1957`): the incremental `bytes_out`
    /// cost is approximated as `huffman_or_raw_length(name) + huffman_or_raw_length(value)`
    /// — i.e. the string bytes alone, ignoring varint prefixes. This is a heuristic check,
    /// not a precise projection: a miss just means the guard fires one line early or late,
    /// and the strategy-chain fallback (retry with indexing disabled) is strictly
    /// non-regressing. Exact wire-byte accounting happens in
    /// [`add_compression_counters`](Self::add_compression_counters) after the line is
    /// planned.
    ///
    /// The guard is disabled (always returns `false`) when `inflation_ratio_max >= 1.0`.
    pub(super) fn would_inflate(&self, name: &[u8], value: &[u8]) -> bool {
        if self.inflation_ratio_max >= 1.0 {
            return false;
        }
        let name_out = to_u32(
            crate::headers::qpack::huffman::encoded_length_if_shorter(name).unwrap_or(name.len()),
        );
        let value_out = to_u32(
            crate::headers::qpack::huffman::encoded_length_if_shorter(value).unwrap_or(value.len()),
        );
        let projected_out = self
            .bytes_out
            .saturating_add(name_out)
            .saturating_add(value_out);
        let projected_in = self
            .bytes_in
            .saturating_add(to_u32(name.len()))
            .saturating_add(to_u32(value.len()));
        if projected_in == 0 {
            return false;
        }
        // f32 precision loss at large counter values is irrelevant here — the rescale
        // caps `bytes_out` at 1000 long before precision degrades, and the ratio threshold
        // tolerates order-of-epsilon noise.
        #[allow(clippy::cast_precision_loss)]
        {
            (projected_out as f32) / (projected_in as f32) > self.inflation_ratio_max
        }
    }

    /// Fold `(name_value_bytes, wire_bytes)` into the running inflation counters for this
    /// connection, rescaling if `bytes_out` is approaching `u32::MAX / 2`. Rescaling
    /// preserves the ratio and keeps the EMA-like memory finite. Called once per header
    /// line from the planner.
    ///
    /// Mirrors ls-qpack's post-encode update (`lsqpack.c:2182-2191`), including the
    /// rescale to 1000 when `bytes_out` crosses the halfway point.
    pub(super) fn add_compression_counters(&mut self, name_value_bytes: u32, wire_bytes: u32) {
        self.bytes_in = self.bytes_in.saturating_add(name_value_bytes);
        self.bytes_out = self.bytes_out.saturating_add(wire_bytes);
        if self.bytes_out > (1u32 << 31) {
            // Rescale to `bytes_out = 1000`, scaling `bytes_in` by the same factor to
            // preserve the ratio. f64 avoids precision loss at large `bytes_out`; the
            // result is a small positive integer so the cast back is safe.
            let ratio = f64::from(self.bytes_in) / f64::from(self.bytes_out);
            let scaled = (ratio * 1000.0).round().clamp(0.0, f64::from(u32::MAX));
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                self.bytes_in = scaled as u32;
            }
            self.bytes_out = 1000;
        }
    }

    /// Post-encode refresh pass: walk the draining region of the table and duplicate
    /// entries that the mnemonic predictor has recently seen, biggest-first. Continues
    /// until no more eligible candidates remain. Returns `true` when at least one
    /// Duplicate was enqueued onto the encoder stream.
    ///
    /// Candidate criteria (all must hold):
    /// - Absolute index is in the draining region (see
    ///   [`draining_frontier_abs_idx`](Self::draining_frontier_abs_idx)).
    /// - The entry's `nameval_hash` appears in the predictor ring — i.e. the encoder has recently
    ///   observed this `(name, value)` pair, so refreshing it is likely to pay off.
    /// - No newer live entry shares the same `(name, value)` pair (checked via the `by_name`
    ///   reverse index) — if a fresher copy exists, this one is redundant.
    /// - [`safe_to_dup`](Self::safe_to_dup) is true with no extra floor (the caller's section refs
    ///   are already in `outstanding_sections` and picked up by `eviction_floor()`).
    ///
    /// The caller must have registered the just-encoded section in `outstanding_sections`
    /// before calling this, so the eviction floor protects any refs that section took.
    ///
    /// Skipped entirely when the mnemonic predictor is disabled: without the predictor
    /// the "recently seen" signal is unavailable, so there is no basis for choosing
    /// candidates. Mirrors ls-qpack's `qenc_dup_draining` at `lsqpack.c:1554-1617`.
    ///
    /// Returns the total encoder-stream bytes enqueued by this pass — consumed by the
    /// phase-5 inflation-ratio counter to account for the cost of background refreshes.
    ///
    /// `extra_floor` is an additional eviction-floor `abs_idx` that combines with the
    /// outstanding-sections floor. The per-header caller passes the in-progress section's
    /// current `min_ref_abs_idx` here, because the section isn't yet registered in
    /// `outstanding_sections` during mid-section invocations — without this the pass
    /// could dup into a position that evicts an entry the section is already referencing.
    pub(super) fn dup_draining_pass(&mut self, extra_floor: Option<u64>) -> u32 {
        if !self.mnemonic_indexing {
            return 0;
        }
        // EMA gate: when the dynamic table is smaller than the typical section length,
        // duplicates can't be referenced before they're re-evicted. Mirrors the
        // suppression at the top of ls-qpack's `qenc_dup_draining`.
        if !self.predictor.allow_dup_draining() {
            return 0;
        }
        let mut added_bytes: u32 = 0;
        while let Some(abs_idx) = self.pick_dup_draining_candidate(extra_floor) {
            if self.duplicate(abs_idx, extra_floor).is_err() {
                // Defensive: `safe_to_dup` gated the choice, but the loop mutates state
                // between iterations and a rare edge case could reject the duplicate.
                // Stop rather than spin.
                break;
            }
            if let Some(wire) = self.pending_ops.back() {
                added_bytes = added_bytes.saturating_add(to_u32(wire.len()));
            }
            #[cfg(test)]
            if let Some(c) = self.strategy_counters.as_mut() {
                c.dup_draining_pass_emits += 1;
            }
        }
        added_bytes
    }

    /// Scan the draining region for the largest entry eligible for a policy-driven
    /// Duplicate. See [`dup_draining_pass`](Self::dup_draining_pass) for the full
    /// criteria; this helper just picks one candidate per call. `extra_floor` is
    /// forwarded to [`safe_to_dup`](Self::safe_to_dup) so mid-section callers preserve
    /// their own in-progress refs during the dup safety check.
    fn pick_dup_draining_candidate(&self, extra_floor: Option<u64>) -> Option<u64> {
        let frontier = self.draining_frontier_abs_idx();
        if frontier == 0 {
            return None;
        }
        let oldest_abs = self.insert_count.saturating_sub(self.entries.len() as u64);
        let mut best: Option<(u64, usize)> = None;
        for (rev_i, entry) in self.entries.iter().rev().enumerate() {
            let abs = oldest_abs + rev_i as u64;
            if abs >= frontier {
                break;
            }
            // Biggest-first tie-break matches ls-qpack: only consider entries strictly
            // larger than the current best. Duplicates consume capacity, so refreshing the
            // biggest draining entry first maximises the retained value per remaining slot.
            if best.is_some_and(|(_, best_size)| best_size >= entry.size) {
                continue;
            }
            let h = MnemonicPredictor::hash(entry.name.as_bytes(), entry.value.as_ref());
            if !self.predictor.seen(h).nameval {
                continue;
            }
            let latest = self
                .by_name
                .get(&entry.name)
                .and_then(|idx| idx.by_value.get(entry.value.as_ref()).copied());
            if latest != Some(abs) {
                continue;
            }
            if !self.safe_to_dup(abs, extra_floor) {
                continue;
            }
            best = Some((abs, entry.size));
        }
        best.map(|(abs, _)| abs)
    }

    /// Smallest absolute index whose entry is *not* draining, per ls-qpack's mnemonic
    /// heuristic (`qenc_entry_is_draining`). Entries with `abs_idx < frontier` are
    /// considered draining — close enough to the oldest end of a near-full table that
    /// referencing them risks pinning an entry the encoder is about to evict. Returns `0`
    /// when no live entry is draining (e.g. a mostly-empty table).
    ///
    /// The per-entry ls-qpack formula is
    /// `dist = when_added_used + (capacity - current_used); draining iff dist < capacity/4`,
    /// where `when_added_used` is the `current_used` value captured right *before* the entry
    /// was inserted (ls-qpack/lsqpack.c:1060) — i.e. the sum of sizes of entries strictly
    /// older than this one (still live, since no evictions have happened in the absence of
    /// dropped bytes).
    ///
    /// We walk oldest-first, maintaining `cumulative = free_space + sum_of_entry_sizes_so_far`.
    /// After adding entry E's size, `cumulative` equals the `dist` value for the *next*
    /// entry (E+1) — so the first time `cumulative >= threshold`, entry E+1 is the smallest
    /// non-draining abs_idx. Return `abs(E) + 1`.
    ///
    /// Off-by-one history: the original implementation returned `abs(E)` here, which is the
    /// *last draining* entry — so `pick_dup_draining_candidate`'s `abs < frontier` check
    /// excluded the largest candidate in the draining region. This was a significant source
    /// of the dup-rate gap vs ls-qpack on fb-resp at (4096,100); see the project memory
    /// entry for details.
    ///
    /// O(draining-region size). Typically a small handful of entries even on a full table;
    /// bounded by `entries.len()`.
    pub(super) fn draining_frontier_abs_idx(&self) -> u64 {
        let threshold = self.capacity / 4;
        let mut cumulative = self.capacity.saturating_sub(self.current_size);
        if cumulative >= threshold {
            return 0;
        }
        let oldest_abs = self.insert_count.saturating_sub(self.entries.len() as u64);
        for (rev_i, entry) in self.entries.iter().rev().enumerate() {
            cumulative = cumulative.saturating_add(entry.size);
            if cumulative >= threshold {
                return oldest_abs + rev_i as u64 + 1;
            }
        }
        // All live entries are draining — frontier sits just past the newest entry so
        // every abs_idx in the table is `< frontier`.
        self.insert_count
    }

    /// Would a [`duplicate`](Self::duplicate) of `src_abs_idx` succeed against the current
    /// table without touching the source or any entry protected by a floor? Pure read —
    /// does not mutate, does not allocate.
    ///
    /// Mirrors ls-qpack's `qenc_safe_to_dup` intuition: simulate the new copy (cost
    /// `src.size`) and greedily evict oldest entries until the table fits, stopping short
    /// of the source itself or the combined eviction floor. `extra_floor` threads the
    /// planner's in-progress `min_ref_abs_idx` through so the pre-check reflects the same
    /// preserve-floor [`duplicate`](Self::duplicate) would see if invoked now.
    ///
    /// Returns `false` when `src_abs_idx` is not currently live.
    pub(super) fn safe_to_dup(&self, src_abs_idx: u64, extra_floor: Option<u64>) -> bool {
        let Some(src) = self.entry_at_abs(src_abs_idx) else {
            return false;
        };
        let src_size = src.size;
        if self.current_size + src_size <= self.capacity {
            return true;
        }
        let floor = combine_floor(self.eviction_floor(), extra_floor);
        let oldest_abs = self.insert_count.saturating_sub(self.entries.len() as u64);
        let mut simulated_used = self.current_size;
        for (rev_i, entry) in self.entries.iter().rev().enumerate() {
            let abs = oldest_abs + rev_i as u64;
            if abs == src_abs_idx {
                return false;
            }
            // Same pin semantics as [`evict_down_to_with_floor`]: block once the simulated
            // eviction reaches the pinned `abs_idx`; entries strictly older than the pin are
            // unpinned and may be evicted to make room.
            if floor.is_some_and(|pin| abs >= pin) {
                return false;
            }
            simulated_used = simulated_used.saturating_sub(entry.size);
            if simulated_used + src_size <= self.capacity {
                return true;
            }
        }
        false
    }

    /// Look up a currently-live entry by its absolute index. Returns `None` if the entry
    /// has already been evicted or the index is past `insert_count`.
    pub(super) fn entry_at_abs(&self, abs_idx: u64) -> Option<&Entry> {
        let oldest_abs = self.insert_count.checked_sub(self.entries.len() as u64)?;
        if abs_idx < oldest_abs || abs_idx >= self.insert_count {
            return None;
        }
        let pos = usize::try_from(self.insert_count - 1 - abs_idx).ok()?;
        self.entries.get(pos)
    }

    /// Whether `stream_id` has at least one outstanding section blocking on an insert that
    /// has not yet been acknowledged (`required_insert_count > known_received_count`).
    pub(super) fn is_stream_blocking(&self, stream_id: u64) -> bool {
        self.outstanding_sections
            .get(&stream_id)
            .is_some_and(|sections| {
                sections
                    .iter()
                    .any(|s| s.required_insert_count > self.known_received_count)
            })
    }

    /// Count of distinct streams with at least one section whose RIC exceeds the current
    /// Known Received Count. RFC 9204 §2.1.2 bounds this count by the peer's advertised
    /// `SETTINGS_QPACK_BLOCKED_STREAMS`.
    pub(super) fn currently_blocked_streams(&self) -> usize {
        let krc = self.known_received_count;
        self.outstanding_sections
            .iter()
            .filter(|(_, sections)| sections.iter().any(|s| s.required_insert_count > krc))
            .count()
    }

    /// The smallest absolute index currently pinned by an outstanding section, or `None` if
    /// no outstanding section references any dynamic entry.
    fn eviction_floor(&self) -> Option<u64> {
        self.outstanding_sections
            .values()
            .flat_map(|sections| sections.iter())
            .filter_map(|s| s.min_ref_abs_idx)
            .min()
    }

    /// Evict oldest entries until `current_size <= target_size`, respecting the eviction
    /// floor from outstanding pinned sections. Returns an error without mutating if a
    /// pinned entry would have to be evicted.
    fn evict_down_to(&mut self, target_size: usize) -> Result<(), H3Error> {
        let floor = self.eviction_floor();
        self.evict_down_to_with_floor(target_size, floor)
    }

    /// Inner eviction loop. Private — callers go through
    /// [`evict_down_to`](Self::evict_down_to) (no preserve floor) or
    /// [`make_room_for`](Self::make_room_for) (size validation + optional preserve floor),
    /// which compute the appropriate floor.
    fn evict_down_to_with_floor(
        &mut self,
        target_size: usize,
        floor: Option<u64>,
    ) -> Result<(), H3Error> {
        let pre_count = self.entries.len();
        let mut result = Ok(());
        while self.current_size > target_size {
            let evicted_abs = self.insert_count - self.entries.len() as u64;
            // `floor` is the smallest pinned `abs_idx`. Entries strictly older (lower abs)
            // are unpinned and FIFO-evictable; the loop walks through them unharmed. Block
            // when the next eviction would touch the pin itself. `>=` (rather than `==`) is
            // defensive — if we ever observe `evicted_abs > pin` we've already evicted a
            // pinned entry, a bug the error here surfaces instead of silently continuing.
            if floor.is_some_and(|pin| evicted_abs >= pin) {
                log::error!(
                    "qpack encoder: eviction blocked (current_size={}, target_size={target_size}, \
                     evicted_abs={evicted_abs}, floor={floor:?})",
                    self.current_size,
                );
                result = Err(H3ErrorCode::QpackEncoderStreamError.into());
                break;
            }
            let Entry { name, value, size } = self.entries.pop_back().expect("current_size > 0");
            self.current_size -= size;
            self.remove_from_reverse_index(&name, value.as_ref(), evicted_abs);
            log::trace!("qpack encoder: evicted entry abs_idx={evicted_abs} size={size}");
        }
        // Sample even on the error path — anything we evicted before hitting the pin is
        // real. Helper no-ops when nothing dropped, matching ls-qpack's
        // `if (dropped && qpe_hist_els)` gate in `qenc_remove_overflow_entries`.
        self.sample_table_size_if_evictions(pre_count);
        result
    }

    /// Funnel for predictor table-size sampling. Call after any code path that may
    /// shrink `self.entries`, passing the entry count from before the operation. Single
    /// place to evolve gating logic (e.g. dropped-bytes weighting) without revisiting
    /// each call site.
    fn sample_table_size_if_evictions(&mut self, pre_count: usize) {
        if pre_count > self.entries.len() {
            let nelem = u32::try_from(self.entries.len()).unwrap_or(u32::MAX);
            self.predictor.sample_table_size(nelem);
        }
    }

    /// Remove an evicted entry's reverse-index slot, respecting the staleness rule: the
    /// per-value slot is only cleared if the stored `abs_idx` still matches (otherwise a newer
    /// duplicate has superseded it). If the evicted entry was the latest for its name,
    /// `latest_any` is recomputed from the remaining values; if no values remain, the entire
    /// [`NameIndex`] is removed.
    fn remove_from_reverse_index(
        &mut self,
        name: &QpackEntryName<'static>,
        value: &[u8],
        evicted_abs: u64,
    ) {
        let Some(name_index) = self.by_name.get_mut(name) else {
            return;
        };
        if name_index.by_value.get(value) == Some(&evicted_abs) {
            name_index.by_value.remove(value);
        }
        let drop_name_entry = if name_index.latest_any == evicted_abs {
            match name_index.by_value.values().copied().max() {
                Some(newest) => {
                    name_index.latest_any = newest;
                    false
                }
                None => true,
            }
        } else {
            false
        };
        if drop_name_entry {
            self.by_name.remove(name);
        }
    }
}

/// Combine two optional eviction-floor `abs_idx` values, taking the more conservative
/// (smaller) one when both are set.
fn combine_floor(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}
