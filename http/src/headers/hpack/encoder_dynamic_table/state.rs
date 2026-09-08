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
    collections::VecDeque,
    fmt::{self, Debug},
};

/// Per-entry overhead used in the size calculation.
const ENTRY_OVERHEAD: usize = 32;

/// `SETTINGS_HEADER_TABLE_SIZE` when the peer's SETTINGS frame omits it (RFC 9113 §6.5.2).
const DEFAULT_HEADER_TABLE_SIZE: usize = 4096;

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
    /// Starts at `min(local_preferred_size, 4096)` — the RFC default the peer's decoder
    /// assumes until it advertises otherwise — and is recomputed by
    /// [`HpackEncoder::set_protocol_max_size`][super::HpackEncoder::set_protocol_max_size]
    /// when an explicit `SETTINGS_HEADER_TABLE_SIZE` arrives.
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
    /// [`HttpConfig::recent_pairs_auto`] captured at construction: when set,
    /// [`set_protocol_max_size`](Self::set_protocol_max_size) derives the ring size and
    /// `seen_k` from the operational table size.
    ///
    /// [`HttpConfig::recent_pairs_auto`]: crate::HttpConfig::recent_pairs_auto
    pub(super) recent_pairs_auto: bool,
    /// Warming-insert sighting threshold: a pair earns an insert on its `seen_k`th
    /// sighting within the ring window (the observer hot-flag can promote earlier).
    pub(super) seen_k: u8,
}

#[derive(Default)]
pub(super) struct NameIndex {
    /// Per-value map of live `abs_idx` values. Keys hash and compare as raw bytes so
    /// the encode path can probe the map with `&[u8]` without allocating.
    pub(super) by_value: HashMap<FieldLineValue<'static>, u64>,
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
                        map.entry(
                            &format_args!("{}", String::from_utf8_lossy(k.as_bytes())),
                            v,
                        );
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
    pub(super) value: FieldLineValue<'static>,
    /// `name.len() + value.len() + 32`.
    pub(super) size: usize,
}

impl Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry")
            .field("name", &self.name)
            .field(
                "value",
                &format_args!("{}", String::from_utf8_lossy(self.value.as_bytes())),
            )
            .field("size", &self.size)
            .finish()
    }
}

impl TableState {
    pub(super) fn new(
        local_preferred_size: usize,
        recent_pairs_size: usize,
        recent_pairs_auto: bool,
    ) -> Self {
        let max_size = local_preferred_size.min(DEFAULT_HEADER_TABLE_SIZE);
        let (recent_pairs, seen_k) = if recent_pairs_auto {
            (
                RecentPairs::with_size(RecentPairs::auto_size(max_size)),
                RecentPairs::auto_seen_k(max_size),
            )
        } else {
            (RecentPairs::with_size(recent_pairs_size), 2)
        };
        Self {
            entries: VecDeque::new(),
            current_size: 0,
            max_size,
            local_preferred_size,
            // The decoder assumes 4096 until told otherwise, so only a smaller working size
            // needs announcing.
            pending_size_update: (max_size != DEFAULT_HEADER_TABLE_SIZE).then_some(max_size),
            insert_count: 0,
            by_name: HashMap::new(),
            accum: ConnectionAccumulator::default(),
            recent_pairs,
            recent_pairs_auto,
            seen_k,
        }
    }

    /// Whether an entry of this name and value would be storable at all under the current
    /// `max_size`. Encoding an insert for an entry that can't be stored would make the
    /// peer's decoder churn its table for a reference we'd never emit.
    pub(super) fn fits(&self, name: &EntryName<'_>, value_len: usize) -> bool {
        name.len() + value_len + ENTRY_OVERHEAD <= self.max_size
    }

    /// Apply peer's advertised `SETTINGS_HEADER_TABLE_SIZE`. Recomputes the operational
    /// `max_size` as `min(local_preferred_size, peer_advertised)`, evicts to fit if
    /// shrinking, and queues a Dynamic Table Size Update for the next encode. When
    /// `recent_pairs_auto` is set, re-derives the recent-pairs ring and `seen_k` from the
    /// new operational size.
    ///
    /// Idempotent: a no-op if the new operational size matches the current one.
    pub(super) fn set_protocol_max_size(&mut self, peer_advertised: usize) {
        let new_max = self.local_preferred_size.min(peer_advertised);
        if new_max == self.max_size {
            return;
        }
        self.max_size = new_max;
        if self.recent_pairs_auto {
            // A mid-connection SETTINGS change discards the ring's sightings along with
            // its sizing. Acceptable: peers rarely resize after startup, and the ring
            // refills within one section's worth of traffic.
            self.recent_pairs = RecentPairs::with_size(RecentPairs::auto_size(new_max));
            self.seen_k = RecentPairs::auto_seen_k(new_max);
        }
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

    /// Insert `(name, value)` at the newest end, evicting oldest entries FIFO until it fits.
    /// Callers check [`fits`](Self::fits) first and send the field without indexing when it
    /// fails, so the §4.4 oversize-clears rule never has to fire here.
    pub(super) fn insert(&mut self, name: EntryName<'_>, value: FieldLineValue<'_>) {
        debug_assert!(
            self.fits(&name, value.len()),
            "encode gates inserts on `fits`"
        );
        let entry_size = name.len() + value.len() + ENTRY_OVERHEAD;
        self.evict_until_fits(entry_size);

        let abs_idx = self.insert_count;
        let name = name.into_owned();
        let value = value.into_shared();
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
