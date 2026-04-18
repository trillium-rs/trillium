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

    /// §3.2.4 Duplicate fast-path for [`insert`](Self::insert). Called only when the smart
    /// picker has already confirmed `abs_idx` is the live entry matching the caller's
    /// `(name, value)` — the source's stored name+value are cloned (cheap `Cow` clones for
    /// the common `'static` case) rather than allocating fresh owned copies from the
    /// caller's borrowed inputs.
    ///
    /// The source `abs_idx` is added to the eviction floor for the duration of
    /// `make_room_for` so it remains live for the post-eviction clone.
    fn duplicate(&mut self, abs_idx: u64, extra_floor: Option<u64>) -> Result<u64, H3Error> {
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
        while self.current_size > target_size {
            let evicted_abs = self.insert_count - self.entries.len() as u64;
            if floor.is_some_and(|min_live| evicted_abs <= min_live) {
                log::error!(
                    "qpack encoder: eviction blocked (current_size={}, target_size={target_size}, \
                     evicted_abs={evicted_abs}, floor={floor:?})",
                    self.current_size,
                );
                return Err(H3ErrorCode::QpackEncoderStreamError.into());
            }
            let Entry { name, value, size } = self.entries.pop_back().expect("current_size > 0");
            self.current_size -= size;
            self.remove_from_reverse_index(&name, value.as_ref(), evicted_abs);
            log::trace!("qpack encoder: evicted entry abs_idx={evicted_abs} size={size}");
        }
        Ok(())
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
