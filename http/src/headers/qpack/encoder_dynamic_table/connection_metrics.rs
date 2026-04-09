//! Per-connection instrumentation for the QPACK observer/priming evaluation.
//!
//! **Research-mode scaffolding, not shipping surface.** The thesis under evaluation is that
//! cross-connection priming can beat ls-qpack-style mnemonic indexing on realistic traffic
//! because priming inserts are *eager* — sent immediately after peer settings and mostly
//! acknowledged by the time any response field section is encoded — whereas ls-qpack's
//! inserts are *load-bearing*, i.e. they sit on the encoder stream concurrent with the
//! response and gate its decoding.
//!
//! Aggregate bytes-on-the-wire hides that distinction. These counters separate eager from
//! load-bearing, track how often primed entries actually get referenced, and emit a
//! per-connection summary on drop so a real-server run against real browsers can be
//! evaluated without a synthetic harness. Expected to be removed once the direction is
//! confirmed or falsified.

use super::super::{FieldLineValue, entry_name::QpackEntryName};
use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
};

/// Per-connection QPACK encoder metrics, aggregated and logged on `EncoderDynamicTable`
/// drop.
///
/// All counters use `Relaxed` ordering — they're monotonic and read only after drop.
#[derive(Debug)]
pub(super) struct ConnectionMetrics {
    /// Number of priming inserts emitted in `initialize_from_peer_settings`.
    pub(super) priming_entries_sent: AtomicU64,
    /// Encoder-stream bytes enqueued by the priming loop. "Eager" in the sense that they
    /// travel during the RTT+handler-time gap before any response is written, so most are
    /// acknowledged before the client starts reading a response field section.
    pub(super) priming_bytes_eager: AtomicU64,
    /// Total dynamic-table references to primed entries across all encoded sections on
    /// this connection. A reference is one occurrence of a `(name, value)` that maps back
    /// to a primed absolute index — whether via an `IndexedDynamic` or a
    /// `LiteralDynamicName` emission.
    pub(super) priming_references_total: AtomicU64,
    /// Number of response field sections encoded on this connection.
    pub(super) sections_total: AtomicU64,
    /// Number of sections that referenced at least one primed entry.
    pub(super) sections_with_primed_reference: AtomicU64,
    /// Total wire bytes of response field sections (the header-block portion — on the
    /// critical path for the peer to decode the response).
    pub(super) field_section_bytes: AtomicU64,
    /// Encoder-stream bytes enqueued during `encode_field_lines` (i.e. concurrent with at
    /// least one in-flight response section). "Load-bearing" in the sense that they gate
    /// the client's ability to decode the section that triggered them: the client's RIC
    /// gate holds until these bytes arrive and are processed.
    pub(super) encoder_stream_bytes_load_bearing: AtomicU64,
    /// Per-primed-entry bookkeeping: name, value, and reference count. Looked up by
    /// absolute table index on each dynamic reference during encode. Name and value are
    /// cloned into `'static` forms here (research-mode instrumentation; allocation cost
    /// is not a concern).
    pub(super) primed_entries: Mutex<HashMap<u64, PrimedEntry>>,
    /// `min(known_received_count, priming_entries_sent)` snapshotted when the *first*
    /// section was encoded — i.e. how many primed inserts had been acknowledged by the
    /// peer's decoder by the time the response started being written. Sentinel `-1`
    /// before the first section is recorded.
    ///
    /// This is the "are eager bytes effectively free yet" measurement: a primed entry
    /// whose insert is already acked behaves like a static-table entry from the client's
    /// latency perspective when this section is decoded. A primed entry whose insert
    /// hasn't been acked yet still works (the encoder stream is FIFO, RIC will hold the
    /// section until insertions catch up) but makes the section blocking until the
    /// insert lands.
    pub(super) primed_acked_at_first_section: AtomicI64,
    /// Running sum across all sections of `min(KRC, primed_count)` at encode time.
    /// Divided by `sections_total` for the per-section average — useful for long-lived
    /// connections where priming was acked partway through.
    pub(super) primed_acked_section_sum: AtomicU64,
}

impl Default for ConnectionMetrics {
    fn default() -> Self {
        Self {
            priming_entries_sent: AtomicU64::new(0),
            priming_bytes_eager: AtomicU64::new(0),
            priming_references_total: AtomicU64::new(0),
            sections_total: AtomicU64::new(0),
            sections_with_primed_reference: AtomicU64::new(0),
            field_section_bytes: AtomicU64::new(0),
            encoder_stream_bytes_load_bearing: AtomicU64::new(0),
            primed_entries: Mutex::new(HashMap::new()),
            // Sentinel: -1 means "first section not yet recorded."
            primed_acked_at_first_section: AtomicI64::new(-1),
            primed_acked_section_sum: AtomicU64::new(0),
        }
    }
}

/// A single primed dynamic-table entry observed over the connection's lifetime. `abs_idx`
/// is the key in [`ConnectionMetrics::primed_entries`]; `name` and `value` are retained
/// only so the drop report can identify which entries paid for themselves.
#[derive(Debug)]
pub(super) struct PrimedEntry {
    pub(super) name: QpackEntryName<'static>,
    pub(super) value: FieldLineValue<'static>,
    pub(super) ref_count: u64,
}

impl ConnectionMetrics {
    /// Record a primed entry after the insert succeeded during
    /// `initialize_from_peer_settings`. `wire_bytes` is the length of the encoder-stream
    /// instruction pushed onto `pending_ops` for this insert.
    pub(super) fn record_primed_insert(
        &self,
        abs_idx: u64,
        name: QpackEntryName<'static>,
        value: FieldLineValue<'static>,
        wire_bytes: u64,
    ) {
        self.priming_entries_sent.fetch_add(1, Ordering::Relaxed);
        self.priming_bytes_eager
            .fetch_add(wire_bytes, Ordering::Relaxed);
        self.primed_entries.lock().unwrap().insert(
            abs_idx,
            PrimedEntry {
                name,
                value,
                ref_count: 0,
            },
        );
    }

    /// Called once per encoded field section. `dynamic_refs` is the flat list of absolute
    /// table indices referenced by this section (indexed-dynamic + literal-dynamic-name).
    /// Each entry in the slice is one reference; repeats are allowed and counted.
    /// `krc_at_encode` is the encoder's `known_received_count` snapshotted under the
    /// state lock during planning — used to compute how much of the priming has been
    /// acked by the time this section was encoded.
    pub(super) fn record_section(
        &self,
        field_section_bytes: u32,
        encoder_stream_bytes: u32,
        dynamic_refs: &[u64],
        krc_at_encode: u64,
    ) {
        let prev_sections = self.sections_total.fetch_add(1, Ordering::Relaxed);
        let sections = prev_sections + 1;
        self.field_section_bytes
            .fetch_add(u64::from(field_section_bytes), Ordering::Relaxed);
        self.encoder_stream_bytes_load_bearing
            .fetch_add(u64::from(encoder_stream_bytes), Ordering::Relaxed);

        let primed_count = self.priming_entries_sent.load(Ordering::Relaxed);
        let primed_acked = krc_at_encode.min(primed_count);
        self.primed_acked_section_sum
            .fetch_add(primed_acked, Ordering::Relaxed);

        if prev_sections == 0 {
            // Safe to do a non-atomic store here: only one section can have prev=0
            // (fetch_add is atomic), so this `if` body executes exactly once.
            #[allow(clippy::cast_possible_wrap)]
            self.primed_acked_at_first_section
                .store(primed_acked as i64, Ordering::Relaxed);
            log::info!(
                target: "qpack_metrics",
                "first section recorded (field_section_bytes={field_section_bytes} \
                 encoder_stream_bytes={encoder_stream_bytes} \
                 dynamic_refs={} primed_acked={primed_acked}/{primed_count})",
                dynamic_refs.len(),
            );
        }

        if !dynamic_refs.is_empty() {
            let mut entries = self.primed_entries.lock().unwrap();
            if !entries.is_empty() {
                let mut primed_refs_this_section: u64 = 0;
                for abs_idx in dynamic_refs {
                    if let Some(entry) = entries.get_mut(abs_idx) {
                        entry.ref_count = entry.ref_count.saturating_add(1);
                        primed_refs_this_section += 1;
                    }
                }
                drop(entries);
                if primed_refs_this_section > 0 {
                    self.sections_with_primed_reference
                        .fetch_add(1, Ordering::Relaxed);
                    self.priming_references_total
                        .fetch_add(primed_refs_this_section, Ordering::Relaxed);
                }
            }
        }

        // Periodic snapshot so long-lived connections (browser pooling, benchmark tools
        // with keep-alive) emit progress without waiting for drop.
        if sections.is_multiple_of(10) {
            self.log_summary_with_prefix("periodic snapshot");
        }
    }

    /// Log a one-line structured summary plus a per-entry breakdown. `prefix` identifies
    /// the reason for logging (drop vs. periodic snapshot) so repeated lines are
    /// distinguishable in a grep.
    pub(super) fn log_summary_with_prefix(&self, prefix: &str) {
        let priming_entries_sent = self.priming_entries_sent.load(Ordering::Relaxed);
        let priming_bytes_eager = self.priming_bytes_eager.load(Ordering::Relaxed);
        let priming_references_total = self.priming_references_total.load(Ordering::Relaxed);
        let sections_total = self.sections_total.load(Ordering::Relaxed);
        let sections_with_primed_reference =
            self.sections_with_primed_reference.load(Ordering::Relaxed);
        let field_section_bytes = self.field_section_bytes.load(Ordering::Relaxed);
        let encoder_stream_bytes_load_bearing = self
            .encoder_stream_bytes_load_bearing
            .load(Ordering::Relaxed);
        let primed_acked_at_first_section =
            self.primed_acked_at_first_section.load(Ordering::Relaxed);
        let primed_acked_section_sum = self.primed_acked_section_sum.load(Ordering::Relaxed);
        let entries = self.primed_entries.lock().unwrap();
        let priming_entries_referenced = entries.values().filter(|e| e.ref_count > 0).count();

        // "n/a" before the first section is recorded, raw count after.
        let first_section_acked = if primed_acked_at_first_section < 0 {
            "n/a".to_string()
        } else {
            format!("{primed_acked_at_first_section}/{priming_entries_sent}")
        };
        let avg_acked = if sections_total > 0 && priming_entries_sent > 0 {
            #[allow(clippy::cast_precision_loss)]
            let avg = primed_acked_section_sum as f64 / sections_total as f64;
            format!("{avg:.2}/{priming_entries_sent}")
        } else {
            "n/a".to_string()
        };

        log::info!(
            target: "qpack_metrics",
            "{prefix}: \
             priming_entries_sent={priming_entries_sent} \
             priming_entries_referenced={priming_entries_referenced} \
             priming_bytes_eager={priming_bytes_eager} \
             priming_references_total={priming_references_total} \
             primed_acked_at_first_section={first_section_acked} \
             primed_acked_per_section_avg={avg_acked} \
             sections_total={sections_total} \
             sections_with_primed_reference={sections_with_primed_reference} \
             field_section_bytes={field_section_bytes} \
             encoder_stream_bytes_load_bearing={encoder_stream_bytes_load_bearing}",
        );
        for (abs_idx, entry) in entries.iter() {
            log::info!(
                target: "qpack_metrics",
                "  primed entry: abs_idx={abs_idx} ref_count={} name={:?} value={:?}",
                entry.ref_count,
                entry.name,
                String::from_utf8_lossy(entry.value.as_bytes()),
            );
        }
    }
}
