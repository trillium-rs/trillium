use crate::{
    h3::{H3Error, H3ErrorCode},
    headers::qpack::entry_name::QpackEntryName,
};
use event_listener::{Event, EventListener};
use std::{
    borrow::Cow,
    collections::{BTreeMap, VecDeque},
    future::Future,
    pin::Pin,
    sync::Mutex,
    task::{Context, Poll, Waker},
};

mod decode;
mod reader;
#[cfg(test)]
mod tests;
mod writer;

/// The QPACK dynamic table for a single HTTP/3 connection (decoder side).
///
/// Entries are added by `run_inbound_encoder` as it processes the peer's encoder stream.
/// Request streams that reference the dynamic table call
/// [`get`](DecoderDynamicTable::get) and await it — it resolves once the required number
/// of inserts have arrived.
#[derive(Debug)]
pub struct DecoderDynamicTable {
    inner: Mutex<DecoderDynamicTableInner>,
    /// Writer-side notifications: new pending section ack, new insert (for Insert Count
    /// Increment), or failure/shutdown. Blocked `get` calls do *not* use this — they have
    /// their own threshold-keyed waker registry inside `inner` so each insert wakes only
    /// the waiters whose Required Insert Count is now met.
    event: Event,
}

#[derive(Debug)]
struct DecoderDynamicTableInner {
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
    /// Waiters blocked inside [`DecoderDynamicTable::get`], keyed by
    /// `(required_insert_count, waiter_id)`. The compound key permits multiple waiters to
    /// share the same threshold while still supporting O(log n) removal on cancellation.
    /// On each insert we drain waiters whose threshold is now met via `extract_if` on the
    /// range `..=(insert_count, u64::MAX)`; we never wake waiters that aren't ready.
    waiters: BTreeMap<(u64, u64), Waker>,
    /// Monotonic counter for waiter IDs. Wraps on overflow, which is safe because the map
    /// is bounded by `max_blocked_streams` at any moment — collision would require one
    /// waiter to remain registered across 2^64 registrations.
    next_waiter_id: u64,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::headers) struct PendingSectionAck {
    pub(in crate::headers) stream_id: u64,
    pub(in crate::headers) required_insert_count: u64,
}

#[derive(Debug, Clone)]
struct DynamicEntry {
    name: QpackEntryName<'static>,
    value: Cow<'static, [u8]>,
    /// `name.len() + value.len() + 32` per RFC 9204 §3.2.1.
    size: usize,
}

/// RAII guard that holds a blocked-stream slot in the [`DecoderDynamicTable`] for the duration of a
/// field-section decode. The slot is released automatically on drop.
#[derive(Debug)]
pub(in crate::headers) struct BlockedStreamGuard<'a>(&'a DecoderDynamicTable);

impl Drop for BlockedStreamGuard<'_> {
    fn drop(&mut self) {
        self.0.decrement_blocked_streams();
    }
}

impl DecoderDynamicTable {
    pub(crate) fn new(max_capacity: usize, max_blocked_streams: usize) -> Self {
        Self {
            inner: Mutex::new(DecoderDynamicTableInner {
                entries: VecDeque::new(),
                max_capacity,
                capacity: 0,
                current_size: 0,
                insert_count: 0,
                failed: None,
                pending_section_acks: Vec::new(),
                max_blocked_streams,
                currently_blocked_streams: 0,
                waiters: BTreeMap::new(),
                next_waiter_id: 0,
            }),
            event: Event::new(),
        }
    }

    /// If this header block's `required_insert_count` is not yet met, attempt to reserve a
    /// blocked-stream slot. Returns a [`BlockedStreamGuard`] that releases the slot on drop.
    ///
    /// Returns `None` if the table already has enough entries (no blocking needed).
    /// Returns `Err(QpackDecompressionFailed)` if the blocked-stream limit would be exceeded.
    pub(in crate::headers) fn try_reserve_blocked_stream(
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

    /// Our advertised `SETTINGS_QPACK_MAX_TABLE_CAPACITY`. Used by the encoder-stream reader
    /// as the per-string length ceiling — any single name/value larger than this is
    /// necessarily invalid (it would produce an entry bigger than we'd ever accept), so
    /// rejecting at read time avoids a peer-triggered allocation path.
    pub(in crate::headers) fn max_capacity(&self) -> usize {
        self.inner.lock().unwrap().max_capacity
    }

    /// Reconstruct the Required Insert Count from its encoded form per RFC 9204 §4.5.1.
    ///
    /// Returns `0` for a static-only field section (encoded value 0). Returns an error if
    /// the encoded value is invalid given the current table state.
    pub(in crate::headers) fn decode_required_insert_count(
        &self,
        encoded: usize,
    ) -> Result<u64, H3ErrorCode> {
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
    pub(in crate::headers) fn set_capacity(&self, new_capacity: usize) -> Result<(), H3Error> {
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
    /// Evicts oldest entries to make room.
    ///
    /// # Errors
    ///
    /// Returns an error if the entry alone exceeds the current capacity (encoder violated
    /// RFC 9204 §3.2.2).
    pub(in crate::headers) fn insert(
        &self,
        name: impl Into<QpackEntryName<'static>>,
        value: Cow<'static, [u8]>,
    ) -> Result<(), H3Error> {
        let name = name.into();
        let entry_size = name.len() + value.as_ref().len() + 32;
        let mut inner = self.inner.lock().unwrap();

        if entry_size > inner.capacity {
            log::error!(
                "Qpack Decoder table entry {name}: (value) exceeded capacity: {entry_size} > {}",
                inner.capacity
            );
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

        let insert_count = inner.insert_count;
        let ready: Vec<Waker> = inner
            .waiters
            .extract_if(..=(insert_count, u64::MAX), |_, _| true)
            .map(|(_, waker)| waker)
            .collect();
        drop(inner);

        for waker in ready {
            waker.wake();
        }
        self.event.notify(usize::MAX);
        Ok(())
    }

    /// Duplicate an existing entry by current relative index (0 = most recently inserted).
    /// Used for the Duplicate encoder instruction (RFC 9204 §3.2.4).
    pub(in crate::headers) fn duplicate(&self, relative_index: usize) -> Result<(), H3Error> {
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
    pub(in crate::headers) fn name_at_relative(
        &self,
        relative_index: usize,
    ) -> Option<QpackEntryName<'static>> {
        self.inner
            .lock()
            .unwrap()
            .entries
            .get(relative_index)
            .map(|e| e.name.clone())
    }

    /// Record that a request stream's header block has been successfully decoded and requires
    /// a Section Acknowledgement. Wakes `run_decoder` to send the instruction.
    pub(in crate::headers) fn acknowledge_section(
        &self,
        stream_id: u64,
        required_insert_count: u64,
    ) {
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
    pub(in crate::headers) fn drain_pending_acks_and_count(&self) -> (Vec<PendingSectionAck>, u64) {
        let mut inner = self.inner.lock().unwrap();
        let acks = inner.pending_section_acks.drain(..).collect();
        let count = inner.insert_count;
        (acks, count)
    }

    /// Create an [`EventListener`] that resolves the next time the table is updated (insert,
    /// capacity change, new pending ack, or failure). Used by `run_decoder` to wake when
    /// there is work to do.
    pub(in crate::headers) fn listen(&self) -> EventListener {
        self.event.listen()
    }

    /// Signal that the encoder stream has failed. Wakes all blocked `get` calls.
    pub(in crate::headers) fn fail(&self, code: H3ErrorCode) {
        let wakers: Vec<Waker> = {
            let mut inner = self.inner.lock().unwrap();
            inner.failed = Some(code);
            std::mem::take(&mut inner.waiters).into_values().collect()
        };
        for waker in wakers {
            waker.wake();
        }
        self.event.notify(usize::MAX);
    }

    /// Look up an entry by its absolute index, waiting until `required_insert_count` entries
    /// have been inserted.
    ///
    /// Returns an error if the encoder stream fails while waiting, or if the entry is absent
    /// after the wait (which would be a protocol error by the encoder).
    pub(in crate::headers) async fn get(
        &self,
        absolute_index: u64,
        required_insert_count: u64,
    ) -> Result<(QpackEntryName<'static>, Cow<'static, [u8]>), H3Error> {
        ThresholdWait {
            table: self,
            threshold: required_insert_count,
            waiter_id: None,
        }
        .await?;

        let inner = self.inner.lock().unwrap();
        if let Some(code) = inner.failed {
            return Err(code.into());
        }
        inner
            .get(absolute_index)
            .ok_or_else(|| H3ErrorCode::QpackDecompressionFailed.into())
    }
}

/// A future that resolves when the dynamic table's `insert_count` reaches `threshold`, or
/// immediately if the encoder stream has failed.
///
/// Each pending waiter occupies a slot in `DecoderDynamicTableInner::waiters` keyed by
/// `(threshold, waiter_id)`. On insert, only waiters whose threshold is met are drained
/// and woken — no spurious wake-and-recheck cycles. On drop, the slot is released so a
/// cancelled decode doesn't leak a stale waker.
struct ThresholdWait<'a> {
    table: &'a DecoderDynamicTable,
    threshold: u64,
    waiter_id: Option<u64>,
}

impl Future for ThresholdWait<'_> {
    type Output = Result<(), H3ErrorCode>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut inner = this.table.inner.lock().unwrap();

        if let Some(code) = inner.failed {
            if let Some(id) = this.waiter_id.take() {
                inner.waiters.remove(&(this.threshold, id));
            }
            return Poll::Ready(Err(code));
        }

        if inner.insert_count >= this.threshold {
            if let Some(id) = this.waiter_id.take() {
                inner.waiters.remove(&(this.threshold, id));
                log::trace!(
                    "QPACK: insert_count {} met required {} — unblocked",
                    inner.insert_count,
                    this.threshold
                );
            }
            return Poll::Ready(Ok(()));
        }

        let id = if let Some(id) = this.waiter_id {
            id
        } else {
            let id = inner.next_waiter_id;
            inner.next_waiter_id = inner.next_waiter_id.wrapping_add(1);
            log::trace!(
                "QPACK: waiting for insert_count >= {} (currently {})",
                this.threshold,
                inner.insert_count
            );
            id
        };
        inner
            .waiters
            .insert((this.threshold, id), cx.waker().clone());
        this.waiter_id = Some(id);
        Poll::Pending
    }
}

impl Drop for ThresholdWait<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.waiter_id.take() {
            // Best-effort cleanup; a poisoned mutex means the table is dead anyway.
            if let Ok(mut inner) = self.table.inner.lock() {
                inner.waiters.remove(&(self.threshold, id));
            }
        }
    }
}

impl DecoderDynamicTableInner {
    fn get(&self, absolute_index: u64) -> Option<(QpackEntryName<'static>, Cow<'static, [u8]>)> {
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
