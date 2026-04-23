//! HPACK dynamic table (RFC 7541 §2.3.2, §4).
//!
//! A FIFO of `(name, value)` entries sized by per-entry byte weight: `name.len() +
//! value.len() + 32` per RFC 7541 §4.1. The newest entry sits at dynamic index 1 (which
//! maps to HPACK absolute index `62`); oldest at the back. Insertion may evict from the
//! tail until the new entry fits; a new entry larger than `max_size` by itself clears the
//! table without being stored (§4.4).
//!
//! This table is agnostic to the protocol-advertised `SETTINGS_HEADER_TABLE_SIZE` upper
//! bound — that check lives at the decoder, which rejects a size-update that exceeds it.

use crate::headers::{entry_name::EntryName, field_section::FieldLineValue};
use std::collections::VecDeque;

/// Per-entry overhead used in the size calculation (RFC 7541 §4.1).
const ENTRY_OVERHEAD: usize = 32;

/// One entry in the dynamic table.
#[derive(Debug, Clone)]
pub(in crate::headers) struct Entry {
    pub(in crate::headers) name: EntryName<'static>,
    pub(in crate::headers) value: FieldLineValue<'static>,
}

impl Entry {
    /// Size contribution of this entry per RFC 7541 §4.1.
    fn size(&self) -> usize {
        self.name.len() + self.value.as_bytes().len() + ENTRY_OVERHEAD
    }
}

/// An HPACK dynamic table.
#[derive(Debug)]
pub(in crate::headers) struct DynamicTable {
    /// Entries in insertion order, newest first. `entries[0]` = dynamic index 1 = HPACK
    /// absolute index 62.
    entries: VecDeque<Entry>,
    /// Sum of `entry.size()` for all live entries.
    size: usize,
    /// Current limit. Shrinks via §6.3 Dynamic Table Size Update; never exceeds the
    /// protocol limit (enforcement at the decoder layer).
    max_size: usize,
}

impl DynamicTable {
    /// Construct a new dynamic table with the given maximum size in bytes.
    pub(in crate::headers) fn new(max_size: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            size: 0,
            max_size,
        }
    }

    /// Look up an entry by 1-based dynamic index (1 = newest).
    ///
    /// HPACK absolute indices above 61 map here via `absolute - 61`.
    pub(in crate::headers) fn get(&self, dyn_index: usize) -> Option<&Entry> {
        if dyn_index == 0 {
            return None;
        }
        self.entries.get(dyn_index - 1)
    }

    /// Apply a §6.3 Dynamic Table Size Update. Evicts oldest entries until the new size
    /// fits.
    pub(in crate::headers) fn set_max_size(&mut self, new_max: usize) {
        self.max_size = new_max;
        self.evict_until_fits(0);
    }

    /// Insert `(name, value)` at the newest end.
    ///
    /// Per RFC 7541 §4.4, if the new entry's §4.1 size alone exceeds `max_size`, the entire
    /// table is cleared and the entry is not stored. Otherwise oldest entries are evicted
    /// until the new entry fits.
    pub(in crate::headers) fn insert(
        &mut self,
        name: EntryName<'static>,
        value: FieldLineValue<'static>,
    ) {
        let entry = Entry { name, value };
        let entry_size = entry.size();

        if entry_size > self.max_size {
            // §4.4: oversized entry — clear the table and drop the insert.
            self.entries.clear();
            self.size = 0;
            return;
        }

        self.evict_until_fits(entry_size);
        self.size += entry_size;
        self.entries.push_front(entry);
    }

    /// Evict from the tail until `self.size + incoming <= self.max_size`.
    fn evict_until_fits(&mut self, incoming: usize) {
        while self.size + incoming > self.max_size {
            let Some(evicted) = self.entries.pop_back() else {
                debug_assert_eq!(self.size, 0, "size desynced from entries");
                break;
            };
            self.size -= evicted.size();
        }
    }
}

/// Test-only accessors used by `tests` here and by HPACK encoder/decoder test modules
/// to assert on post-mutation state. Not used on production paths — the decoder / encoder
/// consult the table via [`DynamicTable::get`] / [`DynamicTable::insert`] / size updates
/// without needing to read these fields directly.
#[cfg(test)]
impl DynamicTable {
    pub(in crate::headers) fn max_size(&self) -> usize {
        self.max_size
    }

    pub(in crate::headers) fn size(&self) -> usize {
        self.size
    }

    pub(in crate::headers) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{DynamicTable, ENTRY_OVERHEAD};
    use crate::{
        KnownHeaderName,
        headers::{entry_name::EntryName, field_section::FieldLineValue},
    };

    fn entry(
        name: KnownHeaderName,
        value: &'static [u8],
    ) -> (EntryName<'static>, FieldLineValue<'static>) {
        (EntryName::Known(name), FieldLineValue::Static(value))
    }

    fn entry_size(name: KnownHeaderName, value: &'static [u8]) -> usize {
        name.as_lower_str().len() + value.len() + ENTRY_OVERHEAD
    }

    #[test]
    fn empty_table() {
        let t = DynamicTable::new(4096);
        assert_eq!(t.len(), 0);
        assert_eq!(t.size(), 0);
        assert_eq!(t.max_size(), 4096);
        assert!(t.get(1).is_none());
        assert!(t.get(0).is_none());
    }

    #[test]
    fn insert_and_index_newest_first() {
        let mut t = DynamicTable::new(4096);
        let (n1, v1) = entry(KnownHeaderName::Date, b"first");
        let (n2, v2) = entry(KnownHeaderName::Etag, b"second");
        t.insert(n1, v1);
        t.insert(n2, v2);

        // Most recently inserted is dynamic index 1.
        assert_eq!(
            t.get(1).unwrap().name,
            EntryName::Known(KnownHeaderName::Etag)
        );
        assert_eq!(
            t.get(2).unwrap().name,
            EntryName::Known(KnownHeaderName::Date)
        );
        assert!(t.get(3).is_none());
        assert!(t.get(0).is_none());
    }

    #[test]
    fn size_accounting_includes_overhead() {
        let mut t = DynamicTable::new(4096);
        let expected_size = entry_size(KnownHeaderName::Date, b"v");
        let (n, v) = entry(KnownHeaderName::Date, b"v");
        t.insert(n, v);
        assert_eq!(t.size(), expected_size);
    }

    #[test]
    fn evicts_oldest_to_fit() {
        // Room for only one ("date" = 4 + 1 + 32 = 37 bytes per entry with 1-byte value).
        let cap = entry_size(KnownHeaderName::Date, b"a");
        let mut t = DynamicTable::new(cap);

        t.insert(
            EntryName::Known(KnownHeaderName::Date),
            FieldLineValue::Static(b"a"),
        );
        t.insert(
            EntryName::Known(KnownHeaderName::Date),
            FieldLineValue::Static(b"b"),
        );

        assert_eq!(t.len(), 1);
        assert_eq!(t.get(1).unwrap().value.as_bytes(), b"b");
    }

    #[test]
    fn oversized_entry_clears_table() {
        // §4.4: inserting an entry larger than max_size evicts everything without
        // storing the new one.
        let mut t = DynamicTable::new(128);
        t.insert(
            EntryName::Known(KnownHeaderName::Date),
            FieldLineValue::Static(b"keep"),
        );
        assert_eq!(t.len(), 1);

        // Entry with a 200-byte value: 32 + "date".len() + 200 = 236 > 128.
        let huge = vec![b'x'; 200];
        t.insert(
            EntryName::Known(KnownHeaderName::Date),
            FieldLineValue::Owned(huge),
        );
        assert_eq!(t.len(), 0, "oversized insert must clear table");
        assert_eq!(t.size(), 0);
    }

    #[test]
    fn set_max_size_evicts_to_fit() {
        let mut t = DynamicTable::new(4096);
        t.insert(
            EntryName::Known(KnownHeaderName::Date),
            FieldLineValue::Static(b"1"),
        );
        t.insert(
            EntryName::Known(KnownHeaderName::Etag),
            FieldLineValue::Static(b"2"),
        );
        t.insert(
            EntryName::Known(KnownHeaderName::Server),
            FieldLineValue::Static(b"3"),
        );
        assert_eq!(t.len(), 3);

        // Shrink to just one entry's worth.
        t.set_max_size(entry_size(KnownHeaderName::Server, b"3"));
        assert_eq!(t.len(), 1);
        assert_eq!(t.get(1).unwrap().value.as_bytes(), b"3", "newest survives");
    }

    #[test]
    fn set_max_size_to_zero_clears() {
        let mut t = DynamicTable::new(4096);
        t.insert(
            EntryName::Known(KnownHeaderName::Date),
            FieldLineValue::Static(b"1"),
        );
        t.set_max_size(0);
        assert_eq!(t.len(), 0);
        assert_eq!(t.size(), 0);
    }

    #[test]
    fn set_max_size_larger_is_noop_on_contents() {
        let mut t = DynamicTable::new(128);
        t.insert(
            EntryName::Known(KnownHeaderName::Date),
            FieldLineValue::Static(b"v"),
        );
        let before = t.size();
        t.set_max_size(8192);
        assert_eq!(t.size(), before);
        assert_eq!(t.len(), 1);
    }
}
