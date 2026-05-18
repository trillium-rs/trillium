//! Internal storage and mutation logic for the HPACK encoder dynamic table.
//!
//! [`TableState`] holds the entries, capacity, reverse-index, and the per-connection
//! observation accumulator. Inserts are inline in the HEADERS block, so this module emits
//! no wire bytes — `encode.rs` writes the wire form, and `insert` only mutates the table.
//!
//! ## Index translation
//!
//! Entries carry an absolute index (`abs_idx`) for stable identity across evictions.
//! The wire form uses a 1-based dynamic index that shifts on every insert — we convert
//! at emit time via [`TableState::dyn_idx_of`] so the reverse index doesn't have to be
//! rewritten on each mutation.

use crate::headers::{
    entry_name::EntryName, field_section::FieldLineValue, header_observer::ConnectionAccumulator,
    recent_pairs::RecentPairs,
};
use hashbrown::HashMap;
use std::{
    borrow::Cow,
    collections::VecDeque,
    fmt::{self, Debug},
};

/// Per-entry overhead used in the size calculation.
const ENTRY_OVERHEAD: usize = 32;

#[derive(Debug)]
pub(super) struct TableState {
    /// Entries in insertion order, newest first. `entries[0]` has dynamic index 1
    /// (HPACK absolute index 62); `entries[i]` has dynamic index `i + 1`.
    pub(super) entries: VecDeque<Entry>,
    /// Sum of `entry.size` for all live entries.
    pub(super) current_size: usize,
    /// Working capacity (bytes). Caps the dynamic table; entries are evicted FIFO when
    /// an insert would exceed it. An insert whose own size exceeds `max_size` clears the
    /// table and is not stored.
    ///
    /// Starts at 0; raised when peer SETTINGS arrives. See
    /// [`HpackEncoder::set_protocol_max_size`][super::HpackEncoder::set_protocol_max_size]
    /// for the "wait for peer" rationale.
    pub(super) max_size: usize,
    /// Encoder's local preferred operational size, fixed at construction. `max_size` is
    /// `min(local_preferred_size, peer_advertised_max)` — `peer_advertised_max` arrives
    /// via [`HpackEncoder::set_protocol_max_size`].
    pub(super) local_preferred_size: usize,
    /// Queued Dynamic Table Size Update. Set whenever `max_size` changes; drained by
    /// [`HpackEncoder::encode`] which prepends the instruction before the first field
    /// representation of the next HEADERS block.
    pub(super) pending_size_update: Option<usize>,
    /// Total entries ever inserted (monotonically increasing). Equals one past the
    /// absolute index of the most-recently inserted entry.
    pub(super) insert_count: u64,
    /// Reverse index for encode-path lookups. Outer map keyed by entry name; each
    /// [`NameIndex`] holds a per-value map (for full-match lookups) and the latest
    /// `abs_idx` across all live entries with this name (for name-only lookups).
    pub(super) by_name: HashMap<EntryName<'static>, NameIndex>,
    /// Per-connection observation accumulator for the cross-connection
    /// [`HeaderObserver`]. Written inline as each line is encoded; folded
    /// into the shared observer once at connection close (in
    /// [`HpackEncoder::Drop`]).
    ///
    /// [`HeaderObserver`]: super::super::super::header_observer::HeaderObserver
    /// [`HpackEncoder::Drop`]: super::HpackEncoder
    pub(super) accum: ConnectionAccumulator,
    /// Per-connection ring of recent (name, value) hashes. Read for the
    /// should-index decision and written immediately afterward as part of
    /// the per-line encode walk.
    pub(super) recent_pairs: RecentPairs,
}

#[derive(Default)]
pub(super) struct NameIndex {
    /// Per-value map of live `abs_idx` values. Values are raw bytes so the encode
    /// path can probe the map with `&[u8]` without allocating.
    pub(super) by_value: HashMap<Cow<'static, [u8]>, u64>,
    /// Latest `abs_idx` across all entries in `by_value`. Recomputed on eviction
    /// when the evicted entry was the latest.
    pub(super) latest_any: u64,
}

impl Debug for NameIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NameIndex")
            .field(
                "by_value",
                &fmt::from_fn(|f| {
                    let mut map = f.debug_map();
                    for (k, v) in &self.by_value {
                        map.entry(&format_args!("{}", String::from_utf8_lossy(k)), v);
                    }
                    map.finish()
                }),
            )
            .field("latest_any", &self.latest_any)
            .finish()
    }
}

#[derive(Clone)]
pub(super) struct Entry {
    pub(super) name: EntryName<'static>,
    pub(super) value: Cow<'static, [u8]>,
    /// `name.len() + value.len() + 32`.
    pub(super) size: usize,
}

impl Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry")
            .field("name", &self.name)
            .field(
                "value",
                &format_args!("{}", String::from_utf8_lossy(&self.value)),
            )
            .field("size", &self.size)
            .finish()
    }
}

impl TableState {
    pub(super) fn new(local_preferred_size: usize, recent_pairs_size: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            current_size: 0,
            max_size: 0,
            local_preferred_size,
            pending_size_update: None,
            insert_count: 0,
            by_name: HashMap::new(),
            accum: ConnectionAccumulator::default(),
            recent_pairs: RecentPairs::with_size(recent_pairs_size),
        }
    }

    /// Apply peer's advertised `SETTINGS_HEADER_TABLE_SIZE`. Recomputes the operational
    /// `max_size` as `min(local_preferred_size, peer_advertised)`, evicts to fit if
    /// shrinking, and queues a Dynamic Table Size Update for the next encode.
    ///
    /// Idempotent: a no-op if the new operational size matches the current one.
    pub(super) fn set_protocol_max_size(&mut self, peer_advertised: usize) {
        let new_max = self.local_preferred_size.min(peer_advertised);
        if new_max == self.max_size {
            return;
        }
        self.max_size = new_max;
        if self.current_size > new_max {
            self.evict_until_fits(0);
        }
        self.pending_size_update = Some(new_max);
    }

    /// Evict oldest entries until `current_size + needed <= max_size`.
    fn evict_until_fits(&mut self, needed: usize) {
        while self.current_size + needed > self.max_size {
            let Some(entry) = self.entries.pop_back() else {
                break;
            };
            let evicted_abs = self.insert_count - self.entries.len() as u64 - 1;
            self.current_size -= entry.size;
            self.remove_from_reverse_index(&entry.name, &entry.value, evicted_abs);
        }
    }

    /// Convert an absolute index to a 1-based dynamic index. Caller has already
    /// verified the entry is live (typically by reading the `abs_idx` from `by_name`).
    pub(super) fn dyn_idx_of(&self, abs_idx: u64) -> usize {
        usize::try_from(self.insert_count - abs_idx).expect("dyn_idx fits in usize")
    }

    /// Returns `Some(dyn_idx)` if `abs_idx` is still live (not evicted), `None`
    /// otherwise. Used by the commit step to decide between an indexed reference
    /// and the pre-baked literal fallback.
    pub(super) fn live_dyn_idx_of(&self, abs_idx: u64) -> Option<usize> {
        let oldest_abs = self.insert_count.checked_sub(self.entries.len() as u64)?;
        if abs_idx < oldest_abs || abs_idx >= self.insert_count {
            return None;
        }
        Some(self.dyn_idx_of(abs_idx))
    }

    /// Insert `(name, value)` at the newest end. An entry whose own size exceeds
    /// `max_size` clears the table and is not stored — the wire instruction has already
    /// been written by the caller, and the decoder applies the same rule on its side, so
    /// the table stays in sync. Otherwise, oldest entries are evicted FIFO until the new
    /// entry fits.
    pub(super) fn insert(&mut self, name: EntryName<'_>, value: FieldLineValue<'_>) {
        let entry_size = name.len() + value.len() + ENTRY_OVERHEAD;

        if entry_size > self.max_size {
            self.entries.clear();
            self.current_size = 0;
            self.by_name.clear();
            return;
        }

        self.evict_until_fits(entry_size);

        let abs_idx = self.insert_count;
        let name = name.into_owned();
        let value = value.into_static();
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
    }

    /// Remove an evicted entry's reverse-index slot, respecting the staleness rule:
    /// the per-value slot is only cleared if the stored `abs_idx` still matches
    /// (otherwise a newer duplicate has superseded it). If the evicted entry was the
    /// latest for its name, `latest_any` is recomputed; if no values remain, the
    /// entire [`NameIndex`] is removed.
    fn remove_from_reverse_index(
        &mut self,
        name: &EntryName<'static>,
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
