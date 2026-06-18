//! Consumer side of RFC 9218 priority for HTTP/3.
//!
//! trillium-http decodes priority signals and emits `(stream_id, priority, is_update)` to a
//! callback registered on the `H3Connection`; it keeps no priority state of its own because the
//! QUIC layer — not trillium-http — owns the send-scheduling bottleneck. This module is that
//! callback's other half. [`transport_priority`] maps the RFC 9218 [`Priority`] to quinn's
//! scheduling value; a [`PriorityRegistry`] maps live request-stream ids to shared
//! [`PrioritySlot`]s, the callback stores the mapped value into the matching slot, and each
//! stream's [`PrioritizedStream`] wrapper applies its slot's value to the underlying QUIC send
//! stream as it writes. The wrapper is the only place that can reprioritize a move-only
//! `SendStream`: the task that owns it.

use crate::{QuicTransportBidi, QuicTransportReceive, QuicTransportSend, Transport};
use atomic_waker::AtomicWaker;
use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    borrow::Cow,
    collections::HashMap,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicI32, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};
use trillium_http::Priority;

/// Map an RFC 9218 [`Priority`] to a quinn send-scheduling value where higher is sent first.
///
/// Each (urgency, incremental) pair gets a distinct level: urgency dominates — a lower urgency
/// number is more urgent, so it maps to a higher value — and within one urgency the
/// non-incremental stream outranks the incremental one. Sixteen levels in all (8 urgencies x 2).
pub(crate) fn transport_priority(priority: Priority) -> i32 {
    -((i32::from(priority.urgency()) << 1) | i32::from(priority.is_incremental()))
}

/// A per-stream priority value shared between the connection's priority callback (writer) and
/// the stream's [`PrioritizedStream`] (reader).
///
/// The `waker` closes the latency gap for a stream parked in `poll_write` because it is
/// flow-control-stalled: without it, a reprioritization would not reach quinn until that stream
/// next polled a write, which under a higher-priority stream's sustained traffic might be a long
/// time. The wrapper registers its task's waker as it writes; a value change wakes it so it
/// re-polls, re-reads the slot, and pushes the new priority down to the QUIC send stream.
#[derive(Debug, Default)]
pub(crate) struct PrioritySlot {
    priority: AtomicI32,
    /// Set once a `PRIORITY_UPDATE` has been applied. After that the request's initial header
    /// priority no longer overwrites it — the latest `PRIORITY_UPDATE` takes precedence over the
    /// header field, regardless of arrival order.
    update_seen: AtomicBool,
    waker: AtomicWaker,
}

impl PrioritySlot {
    /// Apply a received `PRIORITY_UPDATE`: it always wins, and locks out any later initial write.
    /// Wakes the stream's task if the value changed so the new priority reaches quinn promptly.
    fn store_update(&self, priority: i32) {
        self.update_seen.store(true, Ordering::Relaxed);
        self.store_and_wake(priority);
    }

    /// Apply a request's initial header priority, unless a `PRIORITY_UPDATE` has already
    /// superseded it.
    fn store_initial(&self, priority: i32) {
        if !self.update_seen.load(Ordering::Relaxed) {
            self.store_and_wake(priority);
        }
    }

    fn store_and_wake(&self, priority: i32) {
        if self.priority.swap(priority, Ordering::Relaxed) != priority {
            self.waker.wake();
        }
    }

    /// Register the writing task's waker so a later reprioritization can re-poll it.
    fn register(&self, cx: &Context<'_>) {
        self.waker.register(cx.waker());
    }

    fn load(&self) -> i32 {
        self.priority.load(Ordering::Relaxed)
    }
}

/// Upper bound on buffered pre-open `PRIORITY_UPDATE`s, bounding a peer that floods updates for
/// streams it never opens.
const MAX_PENDING_PRIORITY_UPDATES: usize = 128;

/// A connection's live request-stream slots plus a small buffer for updates that arrive before
/// their stream does.
#[derive(Debug, Default)]
struct Streams {
    /// Slots for streams currently being served.
    live: HashMap<u64, Arc<PrioritySlot>>,
    /// `PRIORITY_UPDATE`s received before their stream was accepted, applied once it opens. Only
    /// updates land here — a request's initial header priority is emitted after the stream (and
    /// its slot) already exists, so it always finds a live slot.
    pending: HashMap<u64, i32>,
}

/// A connection's registry of request-stream priority state.
///
/// A cheaply cloneable handle around shared state: the priority callback holds one clone, each
/// accepted request stream registers a slot, and the [`RwLock`] serializes membership and
/// buffering. The hot path — the wrapper reading its slot on each write — touches only the
/// slot's atomics, never this lock.
#[derive(Clone, Debug, Default)]
pub(crate) struct PriorityRegistry(Arc<RwLock<Streams>>);

impl PriorityRegistry {
    /// Register a fresh slot for `stream_id`, draining any `PRIORITY_UPDATE` that arrived before
    /// the stream was accepted, and return the shared handle for its wrapper.
    pub(crate) fn register(&self, stream_id: u64) -> Arc<PrioritySlot> {
        let slot = Arc::<PrioritySlot>::default();
        let mut streams = self.0.write().unwrap();
        if let Some(priority) = streams.pending.remove(&stream_id) {
            log::trace!(
                "H3 stream {stream_id}: applying buffered PRIORITY_UPDATE {priority} on open"
            );
            slot.store_update(priority);
        }
        streams.live.insert(stream_id, slot.clone());
        slot
    }

    /// Drop the slot for `stream_id` once its stream completes.
    pub(crate) fn deregister(&self, stream_id: u64) {
        self.0.write().unwrap().live.remove(&stream_id);
    }

    /// Route a priority signal from the trillium-http callback. A signal for a live stream stores
    /// into its slot (an `is_update` PRIORITY_UPDATE outranks the initial header priority). A
    /// `PRIORITY_UPDATE` for a stream not yet accepted is buffered until it opens; an
    /// initial priority with no live slot — which shouldn't happen — is dropped.
    pub(crate) fn apply(&self, stream_id: u64, priority: i32, is_update: bool) {
        let mut streams = self.0.write().unwrap();
        if let Some(slot) = streams.live.get(&stream_id) {
            if is_update {
                log::trace!("H3 stream {stream_id}: PRIORITY_UPDATE {priority} stored");
                slot.store_update(priority);
            } else {
                log::trace!("H3 stream {stream_id}: initial priority {priority} stored");
                slot.store_initial(priority);
            }
        } else if is_update {
            let at_capacity = streams.pending.len() >= MAX_PENDING_PRIORITY_UPDATES;
            if at_capacity && !streams.pending.contains_key(&stream_id) {
                log::trace!(
                    "H3 stream {stream_id}: dropping PRIORITY_UPDATE {priority} (pending table \
                     full)"
                );
            } else {
                log::trace!(
                    "H3 stream {stream_id}: buffering PRIORITY_UPDATE {priority} (stream not yet \
                     open)"
                );
                streams.pending.insert(stream_id, priority);
            }
        } else {
            log::trace!(
                "H3 stream {stream_id}: dropping initial priority {priority} (no live stream)"
            );
        }
    }
}

/// A QUIC bidi stream that applies RFC 9218 priority changes routed to its [`PrioritySlot`] as
/// it writes. Reading the slot costs one relaxed atomic load per write; `set_priority` reaches
/// the underlying stream only when the value actually changes.
#[derive(Debug)]
pub(crate) struct PrioritizedStream<T> {
    inner: T,
    slot: Arc<PrioritySlot>,
    stream_id: u64,
    applied: Option<i32>,
}

impl<T> PrioritizedStream<T> {
    pub(crate) fn new(inner: T, slot: Arc<PrioritySlot>, stream_id: u64) -> Self {
        Self {
            inner,
            slot,
            stream_id,
            applied: None,
        }
    }
}

impl<T: QuicTransportSend + Unpin> PrioritizedStream<T> {
    fn sync_priority(&mut self) {
        let target = self.slot.load();
        if self.applied != Some(target) {
            log::trace!(
                "H3 stream {}: applying transport priority {target} to send stream",
                self.stream_id
            );
            self.applied = Some(target);
            self.inner.set_priority(target);
        }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for PrioritizedStream<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: QuicTransportSend + Unpin> AsyncWrite for PrioritizedStream<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.slot.register(cx);
        self.sync_priority();
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.slot.register(cx);
        self.sync_priority();
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.slot.register(cx);
        self.sync_priority();
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }
}

impl<T: QuicTransportReceive + Unpin> QuicTransportReceive for PrioritizedStream<T> {
    fn stop(&mut self, code: u64) {
        self.inner.stop(code);
    }
}

impl<T: QuicTransportSend + Unpin> QuicTransportSend for PrioritizedStream<T> {
    fn reset(&mut self, code: u64) {
        self.inner.reset(code);
    }

    fn set_priority(&mut self, priority: i32) {
        self.inner.set_priority(priority);
    }
}

impl<T: Transport + QuicTransportSend> Transport for PrioritizedStream<T> {
    fn set_linger(&mut self, linger: Option<Duration>) -> io::Result<()> {
        self.inner.set_linger(linger)
    }

    fn set_nodelay(&mut self, nodelay: bool) -> io::Result<()> {
        self.inner.set_nodelay(nodelay)
    }

    fn set_ip_ttl(&mut self, ttl: u32) -> io::Result<()> {
        self.inner.set_ip_ttl(ttl)
    }

    fn peer_addr(&self) -> io::Result<Option<SocketAddr>> {
        self.inner.peer_addr()
    }

    fn negotiated_alpn(&self) -> Option<Cow<'_, [u8]>> {
        self.inner.negotiated_alpn()
    }
}

impl<T: QuicTransportBidi + Unpin> QuicTransportBidi for PrioritizedStream<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::{AsyncWriteExt, future::block_on};
    use std::sync::Mutex;

    /// A send half that swallows writes and records each `set_priority` it receives, so a test
    /// can observe exactly when the wrapper reprioritizes the underlying stream.
    struct RecordingSend {
        set_priority_calls: Arc<Mutex<Vec<i32>>>,
    }

    impl AsyncWrite for RecordingSend {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl QuicTransportSend for RecordingSend {
        fn reset(&mut self, _code: u64) {}

        fn set_priority(&mut self, priority: i32) {
            self.set_priority_calls.lock().unwrap().push(priority);
        }
    }

    #[test]
    fn applies_initial_and_updates_only_on_change() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let registry = PriorityRegistry::default();
        let slot = registry.register(4);
        let mut stream = PrioritizedStream::new(
            RecordingSend {
                set_priority_calls: calls.clone(),
            },
            slot,
            4,
        );

        // trillium-http emits the request's initial priority before the first response byte.
        registry.apply(4, -6, false);
        block_on(stream.write_all(b"a")).unwrap();

        // A mid-stream PRIORITY_UPDATE bumps it.
        registry.apply(4, -1, true);
        block_on(stream.write_all(b"b")).unwrap();

        // No change between writes — no redundant set_priority.
        block_on(stream.write_all(b"c")).unwrap();

        assert_eq!(*calls.lock().unwrap(), vec![-6, -1]);
    }

    #[test]
    fn routes_by_stream_id_and_drops_initial_without_a_stream() {
        let registry = PriorityRegistry::default();
        let slot4 = registry.register(4);
        let slot8 = registry.register(8);

        registry.apply(4, -6, true);
        assert_eq!(slot4.load(), -6);
        assert_eq!(
            slot8.load(),
            0,
            "a signal for stream 4 must not touch stream 8"
        );

        // An initial priority naming no live stream is dropped, not buffered.
        registry.apply(999, -3, false);
    }

    #[test]
    fn buffered_update_survives_open_and_outranks_a_later_initial() {
        let registry = PriorityRegistry::default();

        // A PRIORITY_UPDATE arrives before the stream is accepted — it must be buffered.
        registry.apply(4, -1, true);

        // The stream opens; the buffered update seeds the new slot.
        let slot = registry.register(4);
        assert_eq!(slot.load(), -1);

        // The request's initial header priority, emitted at parse, must NOT clobber the update
        // (the latest PRIORITY_UPDATE outranks the header, regardless of order).
        registry.apply(4, -6, false);
        assert_eq!(slot.load(), -1);

        // A subsequent real reprioritization still wins.
        registry.apply(4, -8, true);
        assert_eq!(slot.load(), -8);
    }

    #[test]
    fn update_outranks_initial_when_both_target_a_live_stream() {
        let registry = PriorityRegistry::default();
        let slot = registry.register(4);

        // The PRIORITY_UPDATE lands before the parse-time initial (e.g. it arrived on the control
        // stream between accept and parse). The later initial must not overwrite it.
        registry.apply(4, -1, true);
        registry.apply(4, -6, false);
        assert_eq!(slot.load(), -1);
    }

    #[test]
    fn changed_value_wakes_registered_task_unchanged_does_not() {
        use std::{
            sync::atomic::AtomicUsize,
            task::{Wake, Waker},
        };

        struct CountingWaker(AtomicUsize);
        impl Wake for CountingWaker {
            fn wake(self: Arc<Self>) {
                self.wake_by_ref();
            }

            fn wake_by_ref(self: &Arc<Self>) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let counter = Arc::new(CountingWaker(AtomicUsize::new(0)));
        let waker = Waker::from(counter.clone());
        let cx = Context::from_waker(&waker);
        let wakes = || counter.0.load(Ordering::Relaxed);

        let slot = PrioritySlot::default();

        // A changed value wakes the registered task so it re-polls and re-applies the priority.
        slot.register(&cx);
        slot.store_update(-6);
        assert_eq!(wakes(), 1);

        // `AtomicWaker` consumes the registration on wake; storing the same value re-registers
        // nothing of consequence and must not wake.
        slot.register(&cx);
        slot.store_update(-6);
        assert_eq!(wakes(), 1);

        // A further change wakes again.
        slot.store_update(-1);
        assert_eq!(wakes(), 2);
    }
}
