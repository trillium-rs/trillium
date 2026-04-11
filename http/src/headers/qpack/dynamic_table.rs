use crate::{
    HeaderName, HeaderValue,
    h3::{H3Error, H3ErrorCode},
};
use event_listener::{Event, EventListener};
use std::{collections::VecDeque, sync::Mutex};

/// The QPACK dynamic table for a single HTTP/3 connection (decoder side).
///
/// Entries are added by `run_inbound_encoder` as it processes the peer's encoder stream.
/// Request streams that reference the dynamic table call
/// [`get`](DynamicTable::get) and await it — it resolves once the required number
/// of inserts have arrived.
#[derive(Debug)]
pub struct DynamicTable {
    inner: Mutex<DynamicTableInner>,
    /// Notified on every insert and on failure, waking blocked `get` calls.
    event: Event,
}

#[derive(Debug)]
struct DynamicTableInner {
    /// Entries in insertion order, newest first. `entries[0]` has absolute index
    /// `insert_count - 1`; `entries[i]` has absolute index `insert_count - 1 - i`.
    entries: VecDeque<DynamicEntry>,
    /// `SETTINGS_QPACK_MAX_TABLE_CAPACITY` — what we advertised; fixed for the connection.
    max_capacity: usize,
    /// Current capacity in bytes, set by the peer's encoder via Set Dynamic Table Capacity.
    /// Always ≤ `max_capacity`.
    capacity: usize,
    /// Sum of `entry.size` for all live entries.
    current_size: usize,
    /// Total entries ever inserted (monotonically increasing).
    insert_count: u64,
    /// Set when the encoder stream fails, to propagate the error to blocked waiters.
    failed: Option<H3ErrorCode>,
    /// Pending Section Acknowledgements for streams whose header blocks have been successfully
    /// decoded with a non-zero Required Insert Count. Drained by `run_decoder` to send Section
    /// Acknowledgement instructions; the `required_insert_count` is needed so `run_decoder`
    /// can avoid double-counting inserts that the SA already covers via Known Received Count.
    pending_section_acks: Vec<PendingSectionAck>,
    /// `SETTINGS_QPACK_BLOCKED_STREAMS` — what we advertised; fixed for the connection.
    max_blocked_streams: usize,
    /// Number of streams currently blocked waiting for dynamic table entries.
    currently_blocked_streams: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingSectionAck {
    pub(crate) stream_id: u64,
    pub(crate) required_insert_count: u64,
}

#[derive(Debug, Clone)]
struct DynamicEntry {
    name: HeaderName<'static>,
    value: HeaderValue,
    /// `name.len() + value.len() + 32` per RFC 9204 §3.2.1.
    size: usize,
}

/// RAII guard that holds a blocked-stream slot in the [`DynamicTable`] for the duration of a
/// field-section decode. The slot is released automatically on drop.
#[derive(Debug)]
pub(crate) struct BlockedStreamGuard<'a>(&'a DynamicTable);

impl Drop for BlockedStreamGuard<'_> {
    fn drop(&mut self) {
        self.0.decrement_blocked_streams();
    }
}

impl DynamicTable {
    pub(crate) fn new(max_capacity: usize, max_blocked_streams: usize) -> Self {
        Self {
            inner: Mutex::new(DynamicTableInner {
                entries: VecDeque::new(),
                max_capacity,
                capacity: 0,
                current_size: 0,
                insert_count: 0,
                failed: None,
                pending_section_acks: Vec::new(),
                max_blocked_streams,
                currently_blocked_streams: 0,
            }),
            event: Event::new(),
        }
    }

    /// If this header block's `required_insert_count` is not yet met, attempt to reserve a
    /// blocked-stream slot. Returns a [`BlockedStreamGuard`] that releases the slot on drop.
    ///
    /// Returns `None` if the table already has enough entries (no blocking needed).
    /// Returns `Err(QpackDecompressionFailed)` if the blocked-stream limit would be exceeded.
    pub(crate) fn try_reserve_blocked_stream(
        &self,
        required_insert_count: u64,
    ) -> Result<Option<BlockedStreamGuard<'_>>, H3ErrorCode> {
        let mut inner = self.inner.lock().unwrap();
        if inner.insert_count >= required_insert_count {
            return Ok(None);
        }
        if inner.currently_blocked_streams >= inner.max_blocked_streams {
            return Err(H3ErrorCode::QpackDecompressionFailed);
        }
        inner.currently_blocked_streams += 1;
        Ok(Some(BlockedStreamGuard(self)))
    }

    fn decrement_blocked_streams(&self) {
        self.inner.lock().unwrap().currently_blocked_streams -= 1;
    }

    /// Reconstruct the Required Insert Count from its encoded form per RFC 9204 §4.5.1.
    ///
    /// Returns `0` for a static-only field section (encoded value 0). Returns an error if
    /// the encoded value is invalid given the current table state.
    pub(crate) fn decode_required_insert_count(&self, encoded: usize) -> Result<u64, H3ErrorCode> {
        if encoded == 0 {
            return Ok(0);
        }
        let inner = self.inner.lock().unwrap();
        let max_entries = inner.max_capacity / 32;
        if max_entries == 0 {
            return Err(H3ErrorCode::QpackDecompressionFailed);
        }
        let full_range = 2 * max_entries;
        if encoded > full_range {
            return Err(H3ErrorCode::QpackDecompressionFailed);
        }
        let total_inserts = inner.insert_count;
        let max_value = total_inserts + max_entries as u64;
        let max_wrapped = (max_value / full_range as u64) * full_range as u64;
        let mut ric = max_wrapped + encoded as u64 - 1;
        if ric > max_value {
            if ric < full_range as u64 {
                return Err(H3ErrorCode::QpackDecompressionFailed);
            }
            ric -= full_range as u64;
        }
        if ric == 0 {
            return Err(H3ErrorCode::QpackDecompressionFailed);
        }
        Ok(ric)
    }

    /// Apply a Set Dynamic Table Capacity instruction from the encoder stream.
    ///
    /// Evicts oldest entries that no longer fit. Returns an error if `new_capacity`
    /// exceeds the `max_capacity` we advertised.
    pub(crate) fn set_capacity(&self, new_capacity: usize) -> Result<(), H3Error> {
        let mut inner = self.inner.lock().unwrap();
        if new_capacity > inner.max_capacity {
            return Err(H3ErrorCode::QpackEncoderStreamError.into());
        }
        inner.capacity = new_capacity;
        while inner.current_size > inner.capacity {
            let Some(evicted) = inner.entries.pop_back() else {
                break;
            };
            inner.current_size -= evicted.size;
        }
        Ok(())
    }

    /// Insert a new entry (from an Insert instruction on the encoder stream).
    ///
    /// Evicts oldest entries to make room. Returns an error if the entry alone exceeds
    /// the current capacity (encoder violated RFC 9204 §3.2.2).
    pub(crate) fn insert(
        &self,
        name: HeaderName<'static>,
        value: HeaderValue,
    ) -> Result<(), H3Error> {
        let entry_size = name.as_ref().len() + value.as_ref().len() + 32;
        let mut inner = self.inner.lock().unwrap();
        if entry_size > inner.capacity {
            return Err(H3ErrorCode::QpackEncoderStreamError.into());
        }
        while inner.current_size + entry_size > inner.capacity {
            let Some(evicted) = inner.entries.pop_back() else {
                break;
            };
            inner.current_size -= evicted.size;
        }
        inner.entries.push_front(DynamicEntry {
            name,
            value,
            size: entry_size,
        });
        inner.current_size += entry_size;
        inner.insert_count += 1;
        drop(inner);
        self.event.notify(usize::MAX);
        Ok(())
    }

    /// Duplicate an existing entry by current relative index (0 = most recently inserted).
    /// Used for the Duplicate encoder instruction (RFC 9204 §3.2.4).
    pub(crate) fn duplicate(&self, relative_index: usize) -> Result<(), H3Error> {
        let (name, value) = {
            let inner = self.inner.lock().unwrap();
            inner
                .entries
                .get(relative_index)
                .map(|e| (e.name.clone(), e.value.clone()))
                .ok_or(H3ErrorCode::QpackEncoderStreamError)?
        };
        self.insert(name, value)
    }

    /// Synchronously look up an entry by current relative index (0 = most recently inserted).
    /// Used when decoding an Insert With Name Reference (dynamic) encoder instruction.
    pub(crate) fn name_at_relative(&self, relative_index: usize) -> Option<HeaderName<'static>> {
        self.inner
            .lock()
            .unwrap()
            .entries
            .get(relative_index)
            .map(|e| e.name.clone())
    }

    /// Record that a request stream's header block has been successfully decoded and requires
    /// a Section Acknowledgement. Wakes `run_decoder` to send the instruction.
    pub(crate) fn acknowledge_section(&self, stream_id: u64, required_insert_count: u64) {
        self.inner
            .lock()
            .unwrap()
            .pending_section_acks
            .push(PendingSectionAck {
                stream_id,
                required_insert_count,
            });
        self.event.notify(usize::MAX);
    }

    /// Drain all pending Section Acknowledgements and return the current insert count.
    /// Called by `run_decoder` on each wakeup.
    pub(crate) fn drain_pending_acks_and_count(&self) -> (Vec<PendingSectionAck>, u64) {
        let mut inner = self.inner.lock().unwrap();
        let acks = inner.pending_section_acks.drain(..).collect();
        let count = inner.insert_count;
        (acks, count)
    }

    /// Create an [`EventListener`] that resolves the next time the table is updated (insert,
    /// capacity change, new pending ack, or failure). Used by `run_decoder` to wake when
    /// there is work to do.
    pub(crate) fn listen(&self) -> EventListener {
        self.event.listen()
    }

    /// Signal that the encoder stream has failed. Wakes all blocked `get` calls.
    pub(crate) fn fail(&self, code: H3ErrorCode) {
        self.inner.lock().unwrap().failed = Some(code);
        self.event.notify(usize::MAX);
    }

    /// Look up an entry by its absolute index, waiting until `required_insert_count` entries
    /// have been inserted.
    ///
    /// Returns an error if the encoder stream fails while waiting, or if the entry is absent
    /// after the wait (which would be a protocol error by the encoder).
    pub(crate) async fn get(
        &self,
        absolute_index: u64,
        required_insert_count: u64,
    ) -> Result<(HeaderName<'static>, HeaderValue), H3Error> {
        loop {
            let listener = self.event.listen();
            {
                let inner = self.inner.lock().unwrap();
                if let Some(code) = inner.failed {
                    return Err(code.into());
                }
                if inner.insert_count >= required_insert_count {
                    return inner
                        .get(absolute_index)
                        .ok_or_else(|| H3ErrorCode::QpackDecompressionFailed.into());
                }
                log::trace!(
                    "QPACK: waiting for insert_count >= {required_insert_count} to look up \
                     absolute index {absolute_index} (currently {})",
                    inner.insert_count
                );
            }
            log::trace!(
                "QPACK: blocking on event listener (insert_count={}, \
                 required={required_insert_count}, index={absolute_index})",
                { self.inner.lock().unwrap().insert_count }
            );
            listener.await;
            log::trace!(
                "QPACK: event listener woke (insert_count={}, required={required_insert_count}, \
                 index={absolute_index})",
                { self.inner.lock().unwrap().insert_count }
            );
        }
    }
}

impl DynamicTableInner {
    fn get(&self, absolute_index: u64) -> Option<(HeaderName<'static>, HeaderValue)> {
        // entries[0] = newest = absolute index (insert_count - 1)
        // entries[i] = absolute index (insert_count - 1 - i)
        let i = usize::try_from(
            self.insert_count
                .checked_sub(1)?
                .checked_sub(absolute_index)?,
        )
        .ok()?;
        self.entries
            .get(i)
            .map(|e| (e.name.clone(), e.value.clone()))
    }
}
