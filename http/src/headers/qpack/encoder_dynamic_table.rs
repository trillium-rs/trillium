//! Outbound QPACK dynamic table (RFC 9204 §3.2).
//!
//! Mirror of [`DecoderDynamicTable`](super::decoder_dynamic_table::DecoderDynamicTable) for the
//! *encoder* side of a connection. Mutations are enqueued as already-encoded encoder-stream
//! instructions and drained by
//! [`run_encoder_stream_writer`](super::encoder_stream_writer::run_encoder_stream_writer).
//! Acknowledgements read from the peer's decoder stream by
//! [`run_decoder_stream_reader`](super::decoder_stream_reader::run_decoder_stream_reader)
//! advance the Known Received Count and release pinned references.
//!
//! The policy that decides *which* inserts to make is not modeled here; this commit wires up
//! the infrastructure for wire encoding, bookkeeping, and protocol responses. Tests drive the
//! op queue directly via `enqueue_*` methods.

use crate::{
    HeaderName, HeaderValue,
    h3::{H3Error, H3ErrorCode},
    headers::qpack::{
        ENC_INSTR_DUPLICATE, ENC_INSTR_INSERT_WITH_LITERAL_NAME, ENC_INSTR_INSERT_WITH_NAME_REF,
        ENC_INSTR_LITERAL_NAME_HUFFMAN_FLAG, ENC_INSTR_NAME_REF_STATIC_FLAG,
        ENC_INSTR_SET_DYNAMIC_TABLE_CAPACITY, STRING_HUFFMAN_FLAG, entry_name::QpackEntryName,
        huffman, static_table::static_entry, varint,
    },
};
use event_listener::{Event, EventListener};
use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
};

mod reader;
mod writer;

/// The encoder-side QPACK dynamic table for a single HTTP/3 connection.
///
/// See the module-level documentation for the overall design. Uses interior mutability so
/// it can be shared by `&self` across request-stream tasks and the encoder/decoder stream
/// task handles.
#[derive(Debug)]
pub struct EncoderDynamicTable {
    inner: Mutex<Inner>,
    /// Notified on: new op enqueued, peer ack received, failure. The encoder stream writer
    /// task awaits this to wake and drain `pending_ops`.
    event: Event,
}

#[derive(Debug)]
struct Inner {
    /// Entries in insertion order, newest first. `entries[0]` has absolute index
    /// `insert_count - 1`; `entries[i]` has absolute index `insert_count - 1 - i`.
    entries: VecDeque<Entry>,
    /// Upper bound on `capacity`. Typically `min(our_configured_limit, peer_advertised_max)`.
    /// A `SetCapacity` enqueue exceeding this is a bug.
    max_capacity: usize,
    /// Current working capacity (bytes). Changed by enqueueing a Set Dynamic Table Capacity
    /// instruction; always ≤ `max_capacity`.
    capacity: usize,
    /// Sum of `entry.size` for all live entries.
    current_size: usize,
    /// Total entries ever inserted (monotonically increasing). Equals one past the absolute
    /// index of the most-recently inserted entry.
    insert_count: u64,
    /// Largest `insert_count` value the peer's decoder is known to have processed. Advanced
    /// by Section Acknowledgement and Insert Count Increment instructions. Entries with
    /// absolute index `< known_received_count` are safely referenced by header blocks
    /// without blocking the peer's decoder.
    known_received_count: u64,
    /// Wire-encoded encoder-stream instructions waiting to be written. Each entry is one
    /// full instruction. Drained in FIFO order; the writer must write them in order.
    pending_ops: VecDeque<Vec<u8>>,
    /// Per-stream outstanding header sections. Each section records the entries it pinned.
    /// Drained by Section Acknowledgement (oldest first) and Stream Cancellation (all).
    outstanding_sections: HashMap<u64, VecDeque<SectionRefs>>,
    /// Set when the encoder or decoder stream fails; wakes the writer task so it can exit.
    failed: Option<H3ErrorCode>,
}

#[derive(Debug, Clone)]
struct Entry {
    #[allow(dead_code)] // used once policy integration lands
    name: QpackEntryName,
    #[allow(dead_code)]
    value: HeaderValue,
    /// `name.len() + value.len() + 32` per RFC 9204 §3.2.1.
    size: usize,
}

/// References held by a single outstanding header section. Used to pin entries against
/// eviction until the peer acknowledges the section.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SectionRefs {
    /// Required Insert Count for this section (one past the highest absolute index
    /// referenced). Becomes the new `known_received_count` when this section is acked,
    /// if larger than the current value.
    pub(crate) required_insert_count: u64,
    /// Smallest absolute index referenced by this section, if any. Contributes to the
    /// eviction floor while this section is outstanding. `None` if the section referenced
    /// only static-table entries.
    pub(crate) min_ref_abs_idx: Option<u64>,
}

impl Default for EncoderDynamicTable {
    /// Construct an empty encoder dynamic table. `max_capacity` and the working `capacity`
    /// both start at 0; call [`initialize_from_peer_settings`](Self::initialize_from_peer_settings)
    /// once the peer's `SETTINGS_QPACK_MAX_TABLE_CAPACITY` is known before any inserts.
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                entries: VecDeque::new(),
                max_capacity: 0,
                capacity: 0,
                current_size: 0,
                insert_count: 0,
                known_received_count: 0,
                pending_ops: VecDeque::new(),
                outstanding_sections: HashMap::new(),
                failed: None,
            }),
            event: Event::new(),
        }
    }
}

impl EncoderDynamicTable {
    /// Initialize the table from peer settings. Sets both `max_capacity` and the working
    /// `capacity` to `min(our_max, peer_max)` and, if that value is non-zero, enqueues a
    /// Set Dynamic Table Capacity instruction (RFC 9204 §3.2.1, §4.3.1).
    ///
    /// Must be called exactly once, immediately after the peer's `SETTINGS` frame is parsed
    /// on the control stream.
    pub(crate) fn initialize_from_peer_settings(&self, our_max: usize, peer_max: usize) {
        let chosen = our_max.min(peer_max);
        let mut inner = self.inner.lock().unwrap();
        debug_assert_eq!(
            inner.max_capacity, 0,
            "initialize_from_peer_settings called twice"
        );
        inner.max_capacity = chosen;
        inner.capacity = chosen;
        if chosen > 0 {
            inner.pending_ops.push_back(encode_set_capacity(chosen));
            drop(inner);
            self.event.notify(usize::MAX);
        }
    }

    /// Enqueue a Set Dynamic Table Capacity instruction (RFC 9204 §3.2.1, §4.3.1).
    ///
    /// Evicts oldest entries that no longer fit under the new capacity, respecting the
    /// eviction floor imposed by outstanding pinned sections. Returns an error if
    /// `new_capacity > max_capacity` or if eviction would require dropping a pinned entry.
    #[allow(dead_code)] // exercised by tests; first production caller lands with policy integration
    pub(crate) fn enqueue_set_capacity(&self, new_capacity: usize) -> Result<(), H3Error> {
        let mut inner = self.inner.lock().unwrap();
        if new_capacity > inner.max_capacity {
            log::error!(
                "qpack encoder: set_capacity {} exceeds max_capacity {}",
                new_capacity,
                inner.max_capacity
            );
            return Err(H3ErrorCode::QpackEncoderStreamError.into());
        }
        inner.evict_down_to(new_capacity)?;
        inner.capacity = new_capacity;
        inner
            .pending_ops
            .push_back(encode_set_capacity(new_capacity));
        drop(inner);
        self.event.notify(usize::MAX);
        Ok(())
    }

    /// Enqueue an Insert With Literal Name instruction (RFC 9204 §3.2.3).
    ///
    /// Applies the insertion to local state (evicting oldest entries to make room) and
    /// appends the wire-encoded instruction to the op queue. Returns an error if the
    /// entry alone exceeds `capacity`, or if eviction would require dropping a pinned entry.
    #[allow(dead_code)] // wired into encode path in a follow-up commit
    pub(crate) fn enqueue_insert_literal(
        &self,
        name: HeaderName<'static>,
        value: HeaderValue,
    ) -> Result<u64, H3Error> {
        let entry_size = name.as_ref().len() + value.as_ref().len() + 32;
        let wire = encode_insert_literal(name.as_ref().as_bytes(), value.as_ref());

        let mut inner = self.inner.lock().unwrap();
        if entry_size > inner.capacity {
            log::error!(
                "qpack encoder: insert_literal entry_size {} exceeds capacity {}",
                entry_size,
                inner.capacity
            );
            return Err(H3ErrorCode::QpackEncoderStreamError.into());
        }
        let target = inner.capacity - entry_size;
        inner.evict_down_to(target)?;
        inner.entries.push_front(Entry {
            name: name.into(),
            value,
            size: entry_size,
        });
        inner.current_size += entry_size;
        let abs_idx = inner.insert_count;
        inner.insert_count += 1;
        inner.pending_ops.push_back(wire);
        drop(inner);
        self.event.notify(usize::MAX);
        Ok(abs_idx)
    }

    /// Enqueue an Insert With Name Reference instruction against the QPACK static table
    /// (RFC 9204 §3.2.2). Mirrors [`enqueue_insert_literal`](Self::enqueue_insert_literal)
    /// but borrows the name from a static-table slot instead of sending it on the wire.
    ///
    /// Returns an error if `static_name_index` is out of range, if the resulting entry does
    /// not fit under the current capacity, or if eviction would require dropping a pinned
    /// entry.
    #[allow(dead_code)] // wired into encode path in a follow-up commit
    pub(crate) fn enqueue_insert_with_name_ref_static(
        &self,
        static_name_index: u8,
        value: HeaderValue,
    ) -> Result<u64, H3Error> {
        let (static_name, _default_value) =
            static_entry(usize::from(static_name_index)).map_err(|_| {
                log::error!(
                    "qpack encoder: insert_with_name_ref_static index {} out of range",
                    static_name_index
                );
                H3ErrorCode::QpackEncoderStreamError
            })?;
        let name = QpackEntryName::from(*static_name);
        let entry_size = name.len() + value.as_ref().len() + 32;
        let wire =
            encode_insert_with_name_ref(usize::from(static_name_index), true, value.as_ref());

        let mut inner = self.inner.lock().unwrap();
        if entry_size > inner.capacity {
            log::error!(
                "qpack encoder: insert_with_name_ref_static entry_size {} exceeds capacity {}",
                entry_size,
                inner.capacity
            );
            return Err(H3ErrorCode::QpackEncoderStreamError.into());
        }
        let target = inner.capacity - entry_size;
        inner.evict_down_to(target)?;
        inner.entries.push_front(Entry {
            name,
            value,
            size: entry_size,
        });
        inner.current_size += entry_size;
        let abs_idx = inner.insert_count;
        inner.insert_count += 1;
        inner.pending_ops.push_back(wire);
        drop(inner);
        self.event.notify(usize::MAX);
        Ok(abs_idx)
    }

    /// Enqueue an Insert With Name Reference instruction against an existing dynamic-table
    /// entry (RFC 9204 §3.2.2, T=0). The new entry copies the referenced entry's name.
    ///
    /// Returns an error if `name_abs_idx` is no longer live (already evicted), if the
    /// resulting entry does not fit under the current capacity, or if eviction would
    /// require dropping either a pinned entry or the referenced entry itself.
    #[allow(dead_code)] // wired into encode path in a follow-up commit
    pub(crate) fn enqueue_insert_with_name_ref_dynamic(
        &self,
        name_abs_idx: u64,
        value: HeaderValue,
    ) -> Result<u64, H3Error> {
        let mut inner = self.inner.lock().unwrap();
        let name = match inner.entry_at_abs(name_abs_idx) {
            Some(entry) => entry.name.clone(),
            None => {
                log::error!(
                    "qpack encoder: insert_with_name_ref_dynamic references evicted abs_idx {}",
                    name_abs_idx
                );
                return Err(H3ErrorCode::QpackEncoderStreamError.into());
            }
        };
        let entry_size = name.len() + value.as_ref().len() + 32;
        if entry_size > inner.capacity {
            log::error!(
                "qpack encoder: insert_with_name_ref_dynamic entry_size {} exceeds capacity {}",
                entry_size,
                inner.capacity
            );
            return Err(H3ErrorCode::QpackEncoderStreamError.into());
        }
        // Relative index is computed against `insert_count` at the moment this instruction
        // will be applied by the peer. Eviction does not advance `insert_count`, so this
        // value is stable across the upcoming `evict_down_to_preserving` call.
        let relative_index = inner.insert_count - 1 - name_abs_idx;
        let wire = encode_insert_with_name_ref(
            usize::try_from(relative_index).unwrap_or(usize::MAX),
            false,
            value.as_ref(),
        );
        let target = inner.capacity - entry_size;
        inner.evict_down_to_preserving(target, name_abs_idx)?;
        inner.entries.push_front(Entry {
            name,
            value,
            size: entry_size,
        });
        inner.current_size += entry_size;
        let abs_idx = inner.insert_count;
        inner.insert_count += 1;
        inner.pending_ops.push_back(wire);
        drop(inner);
        self.event.notify(usize::MAX);
        Ok(abs_idx)
    }

    /// Enqueue a Duplicate instruction (RFC 9204 §3.2.4). Re-inserts a copy of an existing
    /// dynamic-table entry at the head of the table, keeping a hot entry alive across
    /// eviction.
    ///
    /// Returns an error if `abs_idx` is no longer live, if the resulting entry does not fit
    /// under the current capacity, or if eviction would require dropping either a pinned
    /// entry or the source entry itself.
    #[allow(dead_code)] // wired into encode path in a follow-up commit
    pub(crate) fn enqueue_duplicate(&self, abs_idx: u64) -> Result<u64, H3Error> {
        let mut inner = self.inner.lock().unwrap();
        let (name, value, entry_size) = match inner.entry_at_abs(abs_idx) {
            Some(entry) => (entry.name.clone(), entry.value.clone(), entry.size),
            None => {
                log::error!(
                    "qpack encoder: duplicate references evicted abs_idx {}",
                    abs_idx
                );
                return Err(H3ErrorCode::QpackEncoderStreamError.into());
            }
        };
        if entry_size > inner.capacity {
            log::error!(
                "qpack encoder: duplicate entry_size {} exceeds capacity {}",
                entry_size,
                inner.capacity
            );
            return Err(H3ErrorCode::QpackEncoderStreamError.into());
        }
        let relative_index = inner.insert_count - 1 - abs_idx;
        let wire = encode_duplicate(usize::try_from(relative_index).unwrap_or(usize::MAX));
        let target = inner.capacity - entry_size;
        inner.evict_down_to_preserving(target, abs_idx)?;
        inner.entries.push_front(Entry {
            name,
            value,
            size: entry_size,
        });
        inner.current_size += entry_size;
        let new_abs_idx = inner.insert_count;
        inner.insert_count += 1;
        inner.pending_ops.push_back(wire);
        drop(inner);
        self.event.notify(usize::MAX);
        Ok(new_abs_idx)
    }

    /// Take all currently-queued encoder-stream instructions. Called by the writer task
    /// each time it wakes; it flushes the returned bytes and then awaits [`listen`](Self::listen).
    pub(crate) fn drain_pending_ops(&self) -> Vec<Vec<u8>> {
        self.inner.lock().unwrap().pending_ops.drain(..).collect()
    }

    /// Create an [`EventListener`] that resolves on the next state change (op enqueued,
    /// peer ack received, or failure).
    pub(crate) fn listen(&self) -> EventListener {
        self.event.listen()
    }

    /// Record a Section Acknowledgement received from the peer's decoder stream
    /// (RFC 9204 §4.4.1). Pops the oldest outstanding section for `stream_id` and, if its
    /// `required_insert_count` exceeds the current known-received count, advances the
    /// latter. Returns an error if no section is outstanding for this stream (protocol
    /// error by the peer).
    pub(crate) fn on_section_ack(&self, stream_id: u64) -> Result<(), H3Error> {
        let mut inner = self.inner.lock().unwrap();
        let section = inner
            .outstanding_sections
            .get_mut(&stream_id)
            .and_then(VecDeque::pop_front)
            .ok_or(H3ErrorCode::QpackDecoderStreamError)?;
        if inner
            .outstanding_sections
            .get(&stream_id)
            .is_some_and(VecDeque::is_empty)
        {
            inner.outstanding_sections.remove(&stream_id);
        }
        if section.required_insert_count > inner.known_received_count {
            inner.known_received_count = section.required_insert_count;
        }
        drop(inner);
        self.event.notify(usize::MAX);
        Ok(())
    }

    /// Record a Stream Cancellation received from the peer's decoder stream
    /// (RFC 9204 §4.4.2). Drops all outstanding sections for `stream_id` without advancing
    /// known-received.
    pub(crate) fn on_stream_cancel(&self, stream_id: u64) {
        let mut inner = self.inner.lock().unwrap();
        inner.outstanding_sections.remove(&stream_id);
        drop(inner);
        self.event.notify(usize::MAX);
    }

    /// Record an Insert Count Increment received from the peer's decoder stream
    /// (RFC 9204 §4.4.3). Advances known-received by `increment`. Returns an error if this
    /// would exceed `insert_count` (protocol error by the peer).
    pub(crate) fn on_insert_count_increment(&self, increment: u64) -> Result<(), H3Error> {
        if increment == 0 {
            return Err(H3ErrorCode::QpackDecoderStreamError.into());
        }
        let mut inner = self.inner.lock().unwrap();
        let new_value = inner
            .known_received_count
            .checked_add(increment)
            .ok_or(H3ErrorCode::QpackDecoderStreamError)?;
        if new_value > inner.insert_count {
            return Err(H3ErrorCode::QpackDecoderStreamError.into());
        }
        inner.known_received_count = new_value;
        drop(inner);
        self.event.notify(usize::MAX);
        Ok(())
    }

    /// Signal that the encoder or decoder stream has failed. Wakes the writer so it can
    /// observe `failed()` and exit.
    pub(crate) fn fail(&self, code: H3ErrorCode) {
        self.inner.lock().unwrap().failed = Some(code);
        self.event.notify(usize::MAX);
    }

    /// Returns `Some(code)` if the table has been marked failed.
    pub(crate) fn failed(&self) -> Option<H3ErrorCode> {
        self.inner.lock().unwrap().failed
    }

    /// The current `insert_count` — total entries ever inserted.
    #[cfg(test)]
    pub(crate) fn insert_count(&self) -> u64 {
        self.inner.lock().unwrap().insert_count
    }

    /// The current known-received count.
    #[cfg(test)]
    pub(crate) fn known_received_count(&self) -> u64 {
        self.inner.lock().unwrap().known_received_count
    }

    /// The number of live entries currently in the table.
    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> usize {
        self.inner.lock().unwrap().entries.len()
    }

    /// Record an outstanding section directly. Test hook used by unit tests that exercise
    /// the decoder-stream reader without running the (not-yet-implemented) encode path.
    #[cfg(test)]
    pub(crate) fn push_outstanding_section_for_test(&self, stream_id: u64, section: SectionRefs) {
        self.inner
            .lock()
            .unwrap()
            .outstanding_sections
            .entry(stream_id)
            .or_default()
            .push_back(section);
    }
}

impl Inner {
    /// Look up a currently-live entry by its absolute index. Returns `None` if the entry
    /// has already been evicted or the index is past `insert_count`.
    fn entry_at_abs(&self, abs_idx: u64) -> Option<&Entry> {
        let oldest_abs = self.insert_count.checked_sub(self.entries.len() as u64)?;
        if abs_idx < oldest_abs || abs_idx >= self.insert_count {
            return None;
        }
        let pos = (self.insert_count - 1 - abs_idx) as usize;
        self.entries.get(pos)
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
    /// floor from outstanding pinned sections. Returns an error without mutating if a pinned
    /// entry would have to be evicted.
    ///
    /// Callers hold the `Inner` lock for the entire operation, so intermediate state is not
    /// observable to other threads. The caller is responsible for any preconditions on
    /// `target_size` (e.g. that a new entry of a given size will actually fit under the
    /// current `capacity`).
    fn evict_down_to(&mut self, target_size: usize) -> Result<(), H3Error> {
        let floor = self.eviction_floor();
        self.evict_down_to_with_floor(target_size, floor)
    }

    /// Like [`evict_down_to`](Self::evict_down_to), but also protects the entry at
    /// `preserve_abs_idx` from eviction. Used when enqueueing an Insert With Name Reference
    /// (dynamic) or a Duplicate instruction, where the new entry references an existing
    /// entry and that existing entry must still be live when the peer processes the
    /// instruction.
    fn evict_down_to_preserving(
        &mut self,
        target_size: usize,
        preserve_abs_idx: u64,
    ) -> Result<(), H3Error> {
        let floor = match self.eviction_floor() {
            Some(pin_floor) => Some(pin_floor.min(preserve_abs_idx)),
            None => Some(preserve_abs_idx),
        };
        self.evict_down_to_with_floor(target_size, floor)
    }

    /// Inner eviction loop. Private — callers go through [`evict_down_to`](Self::evict_down_to)
    /// or [`evict_down_to_preserving`](Self::evict_down_to_preserving), which compute the
    /// appropriate floor.
    fn evict_down_to_with_floor(
        &mut self,
        target_size: usize,
        floor: Option<u64>,
    ) -> Result<(), H3Error> {
        while self.current_size > target_size {
            let oldest_abs = self.insert_count - self.entries.len() as u64;
            if floor.is_some_and(|min_live| oldest_abs <= min_live) {
                log::error!(
                    "qpack encoder: eviction blocked (current_size={}, target_size={}, \
                     oldest_abs={}, floor={:?})",
                    self.current_size,
                    target_size,
                    oldest_abs,
                    floor
                );
                return Err(H3ErrorCode::QpackEncoderStreamError.into());
            }
            let evicted = self.entries.pop_back().expect("current_size > 0");
            self.current_size -= evicted.size;
        }
        Ok(())
    }
}

// --- wire encoders (RFC 9204 §3.2) ---

/// Set Dynamic Table Capacity (§3.2.1): `001xxxxx` with a 5-bit prefix integer.
#[allow(dead_code)] // wired into encode path in a follow-up commit
fn encode_set_capacity(capacity: usize) -> Vec<u8> {
    let mut bytes = varint::encode(capacity, 5);
    bytes[0] |= ENC_INSTR_SET_DYNAMIC_TABLE_CAPACITY;
    bytes
}

/// Insert With Literal Name (§3.2.3): `01HNNNNN` with a 5-bit name-length prefix,
/// followed by the name bytes, then a string literal for the value.
#[allow(dead_code)] // wired into encode path in a follow-up commit
fn encode_insert_literal(name: &[u8], value: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(name.len() + value.len() + 4);

    // Name string (special: 5-bit prefix, H flag at 0x20, type bits 0x40).
    let huffman_name = huffman::encode(name);
    let (name_bytes, name_h) = if huffman_name.len() < name.len() {
        (&huffman_name[..], ENC_INSTR_LITERAL_NAME_HUFFMAN_FLAG)
    } else {
        (name, 0)
    };
    let start = buf.len();
    buf.extend_from_slice(&varint::encode(name_bytes.len(), 5));
    buf[start] |= ENC_INSTR_INSERT_WITH_LITERAL_NAME | name_h;
    buf.extend_from_slice(name_bytes);

    append_value_string_literal(&mut buf, value);

    buf
}

/// Insert With Name Reference (§3.2.2): `1THNNNNN...` — 6-bit prefix integer for the name
/// index (T selects static vs dynamic), followed by a string literal for the value.
#[allow(dead_code)] // wired into encode path in a follow-up commit
fn encode_insert_with_name_ref(name_index: usize, is_static: bool, value: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(value.len() + 4);

    let start = buf.len();
    buf.extend_from_slice(&varint::encode(name_index, 6));
    buf[start] |= ENC_INSTR_INSERT_WITH_NAME_REF
        | if is_static {
            ENC_INSTR_NAME_REF_STATIC_FLAG
        } else {
            0
        };

    append_value_string_literal(&mut buf, value);

    buf
}

/// Duplicate (§3.2.4): `000xxxxx` — 5-bit prefix integer for the relative index.
#[allow(dead_code)] // wired into encode path in a follow-up commit
fn encode_duplicate(relative_index: usize) -> Vec<u8> {
    let mut bytes = varint::encode(relative_index, 5);
    bytes[0] |= ENC_INSTR_DUPLICATE;
    bytes
}

/// Append a standard 7-bit-prefix string literal with Huffman flag to `buf`. Used for the
/// value half of Insert With Literal Name and Insert With Name Reference.
fn append_value_string_literal(buf: &mut Vec<u8>, value: &[u8]) {
    let huffman_value = huffman::encode(value);
    let (value_bytes, value_h) = if huffman_value.len() < value.len() {
        (&huffman_value[..], STRING_HUFFMAN_FLAG)
    } else {
        (value, 0)
    };
    let start = buf.len();
    buf.extend_from_slice(&varint::encode(value_bytes.len(), 7));
    buf[start] |= value_h;
    buf.extend_from_slice(value_bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::qpack::decoder_dynamic_table::DecoderDynamicTable;

    fn hv(s: &str) -> HeaderValue {
        HeaderValue::from(s.as_bytes().to_vec())
    }

    fn hn(s: &str) -> HeaderName<'static> {
        HeaderName::parse(s.as_bytes()).unwrap().into_owned()
    }

    #[test]
    fn set_capacity_encodes_wire_bytes() {
        let table = EncoderDynamicTable::default();
        table.initialize_from_peer_settings(4096, 4096);
        // Initialization emitted a SetCapacity(4096); drop it and test a subsequent shrink.
        let _ = table.drain_pending_ops();
        table.enqueue_set_capacity(1024).unwrap();
        let ops = table.drain_pending_ops();
        assert_eq!(ops.len(), 1);
        // First byte should have the 0x20 prefix bits set.
        assert_eq!(ops[0][0] & 0xE0, ENC_INSTR_SET_DYNAMIC_TABLE_CAPACITY);
        // Round-trip through the decoder-side processor to verify format.
        let decoder_table = DecoderDynamicTable::new(4096, 0);
        decoder_table.set_capacity(4096).unwrap(); // decoder needs room to accept
        let mut stream = &ops[0][..];
        futures_lite::future::block_on(decoder_table.run_reader(&mut stream)).unwrap();
    }

    #[test]
    fn set_capacity_rejects_above_max() {
        let table = EncoderDynamicTable::default();
        table.initialize_from_peer_settings(4096, 4096);
        assert!(table.enqueue_set_capacity(8192).is_err());
    }

    #[test]
    fn insert_literal_applies_and_encodes() {
        let table = EncoderDynamicTable::default();
        table.initialize_from_peer_settings(4096, 4096);
        let abs = table
            .enqueue_insert_literal(hn("x-custom"), hv("hello"))
            .unwrap();
        assert_eq!(abs, 0);
        assert_eq!(table.insert_count(), 1);
        assert_eq!(table.entry_count(), 1);

        // Drain both ops and round-trip through the decoder-side processor.
        let ops = table.drain_pending_ops();
        assert_eq!(ops.len(), 2);
        let mut wire = Vec::new();
        for op in ops {
            wire.extend(op);
        }
        let decoder_table = DecoderDynamicTable::new(4096, 0);
        let mut stream = &wire[..];
        futures_lite::future::block_on(decoder_table.run_reader(&mut stream)).unwrap();
        // Mirror assertion: the decoder side observed one insert.
        let name = decoder_table.name_at_relative(0).unwrap();
        assert_eq!(name.as_ref(), "x-custom");
    }

    #[test]
    fn insert_literal_rejects_oversize() {
        let table = EncoderDynamicTable::default();
        table.initialize_from_peer_settings(64, 64);
        // entry_size = name + value + 32; a 64-byte value forces entry_size > 64.
        let big = hv("x".repeat(64).as_str());
        assert!(table.enqueue_insert_literal(hn("n"), big).is_err());
    }

    #[test]
    fn insert_literal_evicts_oldest_when_unpinned() {
        let table = EncoderDynamicTable::default();
        // Capacity 70 bytes fits exactly one entry of size 64 (1+1+32 = 34 per entry? let me size
        // it explicitly): "a":"v" → 1+1+32 = 34. Two entries = 68, third would evict first.
        table.initialize_from_peer_settings(70, 70);
        table.enqueue_insert_literal(hn("a"), hv("1")).unwrap();
        table.enqueue_insert_literal(hn("b"), hv("2")).unwrap();
        assert_eq!(table.entry_count(), 2);
        table.enqueue_insert_literal(hn("c"), hv("3")).unwrap();
        // Oldest entry evicted; insert_count still monotonic.
        assert_eq!(table.insert_count(), 3);
        assert_eq!(table.entry_count(), 2);
    }

    #[test]
    fn section_ack_advances_known_received() {
        let table = EncoderDynamicTable::default();
        table.initialize_from_peer_settings(4096, 4096);
        table.enqueue_insert_literal(hn("a"), hv("1")).unwrap();
        table.enqueue_insert_literal(hn("b"), hv("2")).unwrap();
        // Simulate a request-stream task registering a section that referenced both entries.
        {
            let mut inner = table.inner.lock().unwrap();
            inner
                .outstanding_sections
                .entry(4)
                .or_default()
                .push_back(SectionRefs {
                    required_insert_count: 2,
                    min_ref_abs_idx: Some(0),
                });
        }
        table.on_section_ack(4).unwrap();
        assert_eq!(table.known_received_count(), 2);
    }

    #[test]
    fn section_ack_without_outstanding_errors() {
        let table = EncoderDynamicTable::default();
        assert!(table.on_section_ack(4).is_err());
    }

    #[test]
    fn stream_cancel_drops_sections_without_advancing() {
        let table = EncoderDynamicTable::default();
        {
            let mut inner = table.inner.lock().unwrap();
            inner
                .outstanding_sections
                .entry(4)
                .or_default()
                .push_back(SectionRefs {
                    required_insert_count: 3,
                    min_ref_abs_idx: Some(0),
                });
        }
        table.on_stream_cancel(4);
        assert_eq!(table.known_received_count(), 0);
        assert!(table.inner.lock().unwrap().outstanding_sections.is_empty());
    }

    #[test]
    fn insert_count_increment_advances_and_bounds() {
        let table = EncoderDynamicTable::default();
        table.initialize_from_peer_settings(4096, 4096);
        table.enqueue_insert_literal(hn("a"), hv("1")).unwrap();
        table.enqueue_insert_literal(hn("b"), hv("2")).unwrap();
        table.on_insert_count_increment(1).unwrap();
        assert_eq!(table.known_received_count(), 1);
        table.on_insert_count_increment(1).unwrap();
        assert_eq!(table.known_received_count(), 2);
        // Cannot go past insert_count
        assert!(table.on_insert_count_increment(1).is_err());
    }

    #[test]
    fn insert_count_increment_rejects_zero() {
        let table = EncoderDynamicTable::default();
        assert!(table.on_insert_count_increment(0).is_err());
    }

    #[test]
    fn pinned_entry_blocks_eviction() {
        let table = EncoderDynamicTable::default();
        table.initialize_from_peer_settings(70, 70);
        table.enqueue_insert_literal(hn("a"), hv("1")).unwrap();
        table.enqueue_insert_literal(hn("b"), hv("2")).unwrap();
        // Pin abs index 0.
        {
            let mut inner = table.inner.lock().unwrap();
            inner
                .outstanding_sections
                .entry(4)
                .or_default()
                .push_back(SectionRefs {
                    required_insert_count: 1,
                    min_ref_abs_idx: Some(0),
                });
        }
        // A third insert would want to evict the oldest entry (abs 0) — pinned → error.
        assert!(table.enqueue_insert_literal(hn("c"), hv("3")).is_err());
    }

    #[test]
    fn fail_sets_failed_state() {
        let table = EncoderDynamicTable::default();
        table.fail(H3ErrorCode::QpackEncoderStreamError);
        assert_eq!(table.failed(), Some(H3ErrorCode::QpackEncoderStreamError));
    }

    #[test]
    fn drain_is_fifo() {
        let table = EncoderDynamicTable::default();
        table.initialize_from_peer_settings(4096, 4096);
        // Initialization emits SetCapacity(4096); add two more to verify FIFO ordering.
        table.enqueue_set_capacity(256).unwrap();
        table.enqueue_set_capacity(1024).unwrap();
        let ops = table.drain_pending_ops();
        assert_eq!(ops.len(), 3);
        // Verify order by re-parsing the 5-bit capacity value.
        let (first, _) = varint::decode(&ops[0], 5).unwrap();
        let (second, _) = varint::decode(&ops[1], 5).unwrap();
        let (third, _) = varint::decode(&ops[2], 5).unwrap();
        assert_eq!(first, 4096);
        assert_eq!(second, 256);
        assert_eq!(third, 1024);
    }

    #[test]
    fn insert_with_name_ref_static_applies_and_encodes() {
        let table = EncoderDynamicTable::default();
        table.initialize_from_peer_settings(4096, 4096);
        // Static index 31 is `accept-encoding: "gzip, deflate, br"`; send a custom value.
        let abs = table
            .enqueue_insert_with_name_ref_static(31, hv("identity"))
            .unwrap();
        assert_eq!(abs, 0);
        assert_eq!(table.insert_count(), 1);

        // Round-trip the SetCapacity + Insert through the decoder.
        let ops = table.drain_pending_ops();
        let mut wire = Vec::new();
        for op in ops {
            wire.extend(op);
        }
        let decoder_table = DecoderDynamicTable::new(4096, 0);
        let mut stream = &wire[..];
        futures_lite::future::block_on(decoder_table.run_reader(&mut stream)).unwrap();
        assert_eq!(
            decoder_table.name_at_relative(0).unwrap().as_ref(),
            "Accept-Encoding"
        );
    }

    #[test]
    fn insert_with_name_ref_static_accepts_pseudo_header() {
        let table = EncoderDynamicTable::default();
        table.initialize_from_peer_settings(4096, 4096);
        // Static index 1 is `:path "/"`. The encoder side just needs to store and emit it;
        // verifying the decoder-side representation is out of scope for this test (see the
        // decoder pseudo-header follow-up task).
        let abs = table
            .enqueue_insert_with_name_ref_static(1, hv("/api/users"))
            .unwrap();
        assert_eq!(abs, 0);
        assert_eq!(table.entry_count(), 1);
        // Two ops queued: the SetCapacity from initialization and the InsertWithNameRef.
        let ops = table.drain_pending_ops();
        assert_eq!(ops.len(), 2);
        // Instruction byte: type=1 (bit 0x80), T=1 static (bit 0x40), 6-bit index=1.
        assert_eq!(ops[1][0], 0xC1);
    }

    #[test]
    fn insert_with_name_ref_static_out_of_range_errors() {
        let table = EncoderDynamicTable::default();
        table.initialize_from_peer_settings(4096, 4096);
        // Static table has 99 entries (indices 0..=98); 200 is out of range.
        assert!(
            table
                .enqueue_insert_with_name_ref_static(200, hv("x"))
                .is_err()
        );
    }

    #[test]
    fn insert_with_name_ref_static_rejects_oversize() {
        let table = EncoderDynamicTable::default();
        // accept-encoding is 15 bytes; entry_size = 15 + value + 32 > 40.
        table.initialize_from_peer_settings(40, 40);
        assert!(
            table
                .enqueue_insert_with_name_ref_static(31, hv("identity"))
                .is_err()
        );
    }

    #[test]
    fn insert_with_name_ref_dynamic_applies_and_encodes() {
        let table = EncoderDynamicTable::default();
        table.initialize_from_peer_settings(4096, 4096);
        let base = table
            .enqueue_insert_literal(hn("x-custom"), hv("first"))
            .unwrap();
        let new_abs = table
            .enqueue_insert_with_name_ref_dynamic(base, hv("second"))
            .unwrap();
        assert_eq!(new_abs, 1);
        assert_eq!(table.insert_count(), 2);

        // Round-trip through the decoder: both entries should land.
        let ops = table.drain_pending_ops();
        let mut wire = Vec::new();
        for op in ops {
            wire.extend(op);
        }
        let decoder_table = DecoderDynamicTable::new(4096, 0);
        let mut stream = &wire[..];
        futures_lite::future::block_on(decoder_table.run_reader(&mut stream)).unwrap();
        // Newest entry is at relative 0.
        assert_eq!(
            decoder_table.name_at_relative(0).unwrap().as_ref(),
            "x-custom"
        );
        assert_eq!(
            decoder_table.name_at_relative(1).unwrap().as_ref(),
            "x-custom"
        );
    }

    #[test]
    fn insert_with_name_ref_dynamic_rejects_evicted_abs_idx() {
        let table = EncoderDynamicTable::default();
        table.initialize_from_peer_settings(4096, 4096);
        // No inserts yet — abs_idx 0 is not live.
        assert!(
            table
                .enqueue_insert_with_name_ref_dynamic(0, hv("v"))
                .is_err()
        );
    }

    #[test]
    fn insert_with_name_ref_dynamic_preserves_referenced_entry() {
        let table = EncoderDynamicTable::default();
        // Capacity exactly fits one entry of size 34 ("a"+"1"+32). A second entry would
        // require evicting the first — which is the one we're referencing.
        table.initialize_from_peer_settings(34, 34);
        let base = table.enqueue_insert_literal(hn("a"), hv("1")).unwrap();
        assert!(
            table
                .enqueue_insert_with_name_ref_dynamic(base, hv("2"))
                .is_err()
        );
    }

    #[test]
    fn duplicate_applies_and_encodes() {
        let table = EncoderDynamicTable::default();
        table.initialize_from_peer_settings(4096, 4096);
        let base = table
            .enqueue_insert_literal(hn("x-custom"), hv("hello"))
            .unwrap();
        let dup = table.enqueue_duplicate(base).unwrap();
        assert_eq!(dup, 1);
        assert_eq!(table.insert_count(), 2);
        assert_eq!(table.entry_count(), 2);

        // Round-trip through the decoder: both entries should land with the same name/value.
        let ops = table.drain_pending_ops();
        let mut wire = Vec::new();
        for op in ops {
            wire.extend(op);
        }
        let decoder_table = DecoderDynamicTable::new(4096, 0);
        let mut stream = &wire[..];
        futures_lite::future::block_on(decoder_table.run_reader(&mut stream)).unwrap();
        assert_eq!(
            decoder_table.name_at_relative(0).unwrap().as_ref(),
            "x-custom"
        );
        assert_eq!(
            decoder_table.name_at_relative(1).unwrap().as_ref(),
            "x-custom"
        );
    }

    #[test]
    fn duplicate_rejects_evicted_abs_idx() {
        let table = EncoderDynamicTable::default();
        table.initialize_from_peer_settings(4096, 4096);
        assert!(table.enqueue_duplicate(0).is_err());
    }

    #[test]
    fn duplicate_preserves_referenced_entry() {
        let table = EncoderDynamicTable::default();
        table.initialize_from_peer_settings(34, 34);
        let base = table.enqueue_insert_literal(hn("a"), hv("1")).unwrap();
        assert!(table.enqueue_duplicate(base).is_err());
    }
}
