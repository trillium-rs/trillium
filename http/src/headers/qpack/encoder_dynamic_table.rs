//! Outbound QPACK dynamic table (RFC 9204 §3.2).
//!
//! Mirror of [`DecoderDynamicTable`](super::decoder_dynamic_table::DecoderDynamicTable) for the *encoder* side of a
//! connection. Mutations are enqueued as already-encoded encoder-stream instructions and
//! drained by
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
        ENC_INSTR_INSERT_WITH_LITERAL_NAME, ENC_INSTR_LITERAL_NAME_HUFFMAN_FLAG,
        ENC_INSTR_SET_DYNAMIC_TABLE_CAPACITY, STRING_HUFFMAN_FLAG, huffman, varint,
    },
};
use event_listener::{Event, EventListener};
use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
};

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
    name: HeaderName<'static>,
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

impl EncoderDynamicTable {
    /// Construct an empty encoder dynamic table with the given maximum capacity.
    ///
    /// `max_capacity` is the hard upper bound — typically the minimum of what we're willing
    /// to allocate and what the peer advertised as its `SETTINGS_QPACK_MAX_TABLE_CAPACITY`.
    /// The initial working capacity is 0; enqueue a Set Dynamic Table Capacity instruction
    /// via [`enqueue_set_capacity`](Self::enqueue_set_capacity) before any inserts.
    pub(crate) fn new(max_capacity: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                entries: VecDeque::new(),
                max_capacity,
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

    /// Enqueue a Set Dynamic Table Capacity instruction (RFC 9204 §3.2.1, §4.3.1).
    ///
    /// Evicts oldest entries that no longer fit under the new capacity, respecting the
    /// eviction floor imposed by outstanding pinned sections. Returns an error if
    /// `new_capacity > max_capacity` or if eviction would require dropping a pinned entry.
    #[allow(dead_code)] // wired into encode path in a follow-up commit
    pub(crate) fn enqueue_set_capacity(&self, new_capacity: usize) -> Result<(), H3Error> {
        let mut inner = self.inner.lock().unwrap();
        if new_capacity > inner.max_capacity {
            return Err(H3ErrorCode::QpackEncoderStreamError.into());
        }
        let floor = inner.eviction_floor();
        // Evict as needed to fit the new capacity.
        while inner.current_size > new_capacity {
            // The next entry we would evict is the oldest — absolute index `insert_count -
            // entries.len()`.
            let oldest_abs = inner.insert_count - inner.entries.len() as u64;
            if floor.is_some_and(|min_pinned| oldest_abs <= min_pinned) {
                return Err(H3ErrorCode::QpackEncoderStreamError.into());
            }
            let evicted = inner.entries.pop_back().expect("current_size > 0");
            inner.current_size -= evicted.size;
        }
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
            return Err(H3ErrorCode::QpackEncoderStreamError.into());
        }
        let floor = inner.eviction_floor();
        while inner.current_size + entry_size > inner.capacity {
            let oldest_abs = inner.insert_count - inner.entries.len() as u64;
            if floor.is_some_and(|min_pinned| oldest_abs <= min_pinned) {
                return Err(H3ErrorCode::QpackEncoderStreamError.into());
            }
            let evicted = inner.entries.pop_back().expect("entries non-empty");
            inner.current_size -= evicted.size;
        }
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
    /// The smallest absolute index currently pinned by an outstanding section, or `None` if
    /// no outstanding section references any dynamic entry.
    #[allow(dead_code)] // called from enqueue paths wired in a follow-up commit
    fn eviction_floor(&self) -> Option<u64> {
        self.outstanding_sections
            .values()
            .flat_map(|sections| sections.iter())
            .filter_map(|s| s.min_ref_abs_idx)
            .min()
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

    // Value string (standard 7-bit prefix literal).
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

    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::qpack::{
        ENC_INSTR_INSERT_WITH_NAME_REF, decoder_dynamic_table::DecoderDynamicTable,
        encoder_stream::process_encoder_stream,
    };

    fn hv(s: &str) -> HeaderValue {
        HeaderValue::from(s.as_bytes().to_vec())
    }

    fn hn(s: &str) -> HeaderName<'static> {
        HeaderName::parse(s.as_bytes()).unwrap().into_owned()
    }

    #[test]
    fn set_capacity_encodes_wire_bytes() {
        let table = EncoderDynamicTable::new(4096);
        table.enqueue_set_capacity(1024).unwrap();
        let ops = table.drain_pending_ops();
        assert_eq!(ops.len(), 1);
        // First byte should have the 0x20 prefix bits set.
        assert_eq!(ops[0][0] & 0xE0, ENC_INSTR_SET_DYNAMIC_TABLE_CAPACITY);
        // Round-trip through the decoder-side processor to verify format.
        let decoder_table = DecoderDynamicTable::new(4096, 0);
        decoder_table.set_capacity(4096).unwrap(); // decoder needs room to accept
        let mut stream = &ops[0][..];
        futures_lite::future::block_on(process_encoder_stream(&mut stream, &decoder_table))
            .unwrap();
    }

    #[test]
    fn set_capacity_rejects_above_max() {
        let table = EncoderDynamicTable::new(4096);
        assert!(table.enqueue_set_capacity(8192).is_err());
    }

    #[test]
    fn insert_literal_applies_and_encodes() {
        let table = EncoderDynamicTable::new(4096);
        table.enqueue_set_capacity(4096).unwrap();
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
        futures_lite::future::block_on(process_encoder_stream(&mut stream, &decoder_table))
            .unwrap();
        // Mirror assertion: the decoder side observed one insert.
        let name = decoder_table.name_at_relative(0).unwrap();
        assert_eq!(name.as_ref(), "x-custom");
    }

    #[test]
    fn insert_literal_rejects_oversize() {
        let table = EncoderDynamicTable::new(4096);
        table.enqueue_set_capacity(64).unwrap();
        // entry_size = name + value + 32; a 64-byte value forces entry_size > 64.
        let big = hv("x".repeat(64).as_str());
        assert!(table.enqueue_insert_literal(hn("n"), big).is_err());
    }

    #[test]
    fn insert_literal_evicts_oldest_when_unpinned() {
        let table = EncoderDynamicTable::new(4096);
        // Capacity 70 bytes fits exactly one entry of size 64 (1+1+32 = 34 per entry? let me size
        // it explicitly): "a":"v" → 1+1+32 = 34. Two entries = 68, third would evict first.
        table.enqueue_set_capacity(70).unwrap();
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
        let table = EncoderDynamicTable::new(4096);
        table.enqueue_set_capacity(4096).unwrap();
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
        let table = EncoderDynamicTable::new(4096);
        assert!(table.on_section_ack(4).is_err());
    }

    #[test]
    fn stream_cancel_drops_sections_without_advancing() {
        let table = EncoderDynamicTable::new(4096);
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
        let table = EncoderDynamicTable::new(4096);
        table.enqueue_set_capacity(4096).unwrap();
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
        let table = EncoderDynamicTable::new(4096);
        assert!(table.on_insert_count_increment(0).is_err());
    }

    #[test]
    fn pinned_entry_blocks_eviction() {
        let table = EncoderDynamicTable::new(4096);
        table.enqueue_set_capacity(70).unwrap();
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
        let table = EncoderDynamicTable::new(4096);
        table.fail(H3ErrorCode::QpackEncoderStreamError);
        assert_eq!(table.failed(), Some(H3ErrorCode::QpackEncoderStreamError));
    }

    #[test]
    fn drain_is_fifo() {
        let table = EncoderDynamicTable::new(4096);
        table.enqueue_set_capacity(256).unwrap();
        table.enqueue_set_capacity(1024).unwrap();
        let ops = table.drain_pending_ops();
        assert_eq!(ops.len(), 2);
        // Verify order by re-parsing the 5-bit capacity value.
        let (first, _) = varint::decode(&ops[0], 5).unwrap();
        let (second, _) = varint::decode(&ops[1], 5).unwrap();
        assert_eq!(first, 256);
        assert_eq!(second, 1024);
    }

    #[test]
    fn unused_instruction_constant_is_referenced() {
        // Silences an unused-import warning until the next commit wires in
        // Insert With Name Reference.
        let _ = ENC_INSTR_INSERT_WITH_NAME_REF;
    }
}
