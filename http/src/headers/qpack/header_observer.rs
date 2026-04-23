//! Cross-connection header observer.
//!
//! Tracks the distribution of `(name, value)` pairs a peer emits across many connections so
//! that each new connection's dynamic table can be pre-warmed with the pairs most likely to
//! be referenced. Private to `trillium-http`; never appears in a public signature.
//!
//! The observer targets **slow vocabulary** — values that stay stable across many
//! connections (a server's `server:` banner, a deployment's common `content-type`, hot
//! `:path`s, etc.). Within-connection repetition at the timescale of a single HTTP/3
//! stream is handled separately by the per-connection indexing layer; the observer doesn't
//! try to compete on that axis.
//!
//! Storage key is `(EntryName<'static>, FieldLineValue<'static>)` — the same types
//! the encoder's dynamic table already uses. `EntryName::Pseudo` lets the observer
//! track hot `:path`/`:scheme`/`:authority` values that aren't in the static table.
//!
//! Role isolation: each hop-and-direction pair gets its own observer (see
//! `HttpContext::__isolate_qpack_observer`). A reverse proxy's inbound server observer is
//! distinct from its outbound client observer, so forwarded `authorization`/`cookie`
//! values cannot reach the QPACK state of unrelated clients.
//!
//! ## How the counts work
//!
//! Each entry carries an **EMA-decayed count** of sections that observed the pair, updated
//! lazily on touch. A parallel `total_ema` counts all observed sections with the same
//! decay. Because both use the same decay factor, their ratio stays in `[0, 1]` and
//! answers "what fraction of recent sections included this pair" directly.
//!
//! Priming picks the pairs whose fraction exceeds a threshold (default 30 %) ranked by
//! `count × expected-bytes-saved`, until the capacity budget is spent.
//!
//! "Sections," not "connections": one tick per encoded response field section regardless
//! of which connection it belongs to. Counting by connection would over-trust connection
//! boundaries — a reverse proxy aggregates many distinct logical peers into one connection
//! to us, and a chatty single client looks the same shape as a quiet one. Section-level
//! counting is the simplest definition of "the distribution of outbound headers" and
//! generalizes cleanly across deployment shapes.

use super::{FieldLineValue, static_table};
use crate::{HttpConfig, KnownHeaderName, headers::entry_name::EntryName};
use std::{collections::HashMap, sync::Mutex};

/// Hardcoded threshold — a pair must have appeared in at least this fraction of recent
/// connections (as measured by the EMA ratio) to be considered for priming. Room for a
/// public knob later if phase-B measurement suggests it.
const MIN_PRIMING_FRACTION: f64 = 0.30;

/// Below this many observed connections (monotonic tick, not EMA), priming is suppressed
/// outright — EMA ratios are too noisy on tiny samples. "Warm-up" window for the observer.
/// Using tick rather than `total_ema` keeps this threshold meaningful regardless of the
/// configured half-life (short half-lives saturate below a small asymptote).
///
/// **Research-mode override**: set to 0 for the current observer-evaluation work. The
/// quality filter this provides is real but makes iterative testing impractical (hundreds
/// of distinct connections can't be driven from a browser); re-raise once the mechanism
/// is validated.
const WARMUP_MIN_TICKS: u64 = 0;

/// RFC 9204 §3.2.1 per-entry overhead in the dynamic table (entry size = overhead +
/// name bytes + value bytes).
const ENTRY_OVERHEAD: u32 = 32;

/// Per-server tracker of header-pair frequencies, consulted when priming a new
/// connection's dynamic table.
#[derive(Debug)]
pub(crate) struct HeaderObserver {
    inner: Mutex<ObserverInner>,
    config: ObserverConfig,
}

impl Default for HeaderObserver {
    fn default() -> Self {
        Self::from_http_config(&HttpConfig::DEFAULT)
    }
}

/// Storage key for the observer's frequency map.
///
/// `(name, Some(value))` entries track full `(name, value)` pair frequency — candidates
/// for full-pair priming. Suppressed for sensitive headers and Date (see
/// [`value_is_uncacheable`]) so we never cache values that shouldn't outlive a connection.
///
/// `(name, None)` entries track name-only frequency, summed across all values seen for
/// that name. Always tracked, including for sensitive headers — priming
/// `(authorization, "")` saves name bytes per future use without caching any value.
type ObserverKey = (EntryName<'static>, Option<FieldLineValue<'static>>);

#[derive(Debug, Default)]
struct ObserverInner {
    entries: HashMap<ObserverKey, EntryStats>,
    /// Monotonic counter of sections observed (used as the EMA "time" axis).
    tick: u64,
    /// EMA-decayed count of all observed sections. Denominator in the fraction.
    total_ema: f64,
    /// EMA-decayed average of headers-per-section. Updated at the *start* of each
    /// section by sampling the prior section's count. Queried by new connections via
    /// [`HeaderObserver::suggested_ring_size`] to pre-size their per-connection ring.
    /// Same EMA decay factor as `total_ema`. `None` until the first sample arrives.
    headers_per_section_ema: Option<f64>,
    /// Number of [`HeaderObserver::record_observation`] calls in the section currently
    /// being recorded. Sampled into `headers_per_section_ema` at the next
    /// [`HeaderObserver::record_section_start`] and reset.
    headers_in_current_section: u32,
}

/// Runtime-immutable observer settings. Derived once from [`HttpConfig`] at construction.
#[derive(Debug, Clone, Copy)]
struct ObserverConfig {
    max_entries: u32,
    /// EMA decay factor applied once per connection tick. Derived from the configured
    /// half-life: `0.5 ^ (1 / half_life)`.
    decay_per_tick: f64,
}

impl ObserverConfig {
    fn from_http_config(config: &HttpConfig) -> Self {
        let half_life = config.h3_qpack_header_observer_half_life_sections.max(1);
        Self {
            max_entries: config.h3_qpack_header_observer_max_entries,
            decay_per_tick: 0.5_f64.powf(f64::from(half_life).recip()),
        }
    }
}

#[derive(Debug, Default)]
struct EntryStats {
    /// EMA-decayed count of connections in which this pair has been observed.
    count: f64,
    /// Tick at which `count` was last updated — used to lazily apply decay on touch.
    last_updated_tick: u64,
}

impl EntryStats {
    /// Advance lazy decay to `tick` and return the effective count at that tick.
    fn effective_count_at(&self, tick: u64, decay_per_tick: f64) -> f64 {
        let elapsed = tick.saturating_sub(self.last_updated_tick);
        if elapsed == 0 {
            self.count
        } else {
            #[allow(clippy::cast_precision_loss)]
            let factor = decay_per_tick.powf(elapsed as f64);
            self.count * factor
        }
    }
}

impl HeaderObserver {
    /// Construct an observer with tuning derived from `config`.
    pub(crate) fn from_http_config(config: &HttpConfig) -> Self {
        Self {
            inner: Mutex::new(ObserverInner::default()),
            config: ObserverConfig::from_http_config(config),
        }
    }

    /// Advance the tick and update the EMA denominator. Caller invokes this once per
    /// encoded response section, before reporting that section's individual headers via
    /// [`record_observation`](Self::record_observation). Also samples the prior
    /// section's header count into `headers_per_section_ema` (skipped on the very first
    /// call, when there is no prior section).
    pub(in crate::headers) fn record_section_start(&self) {
        let mut inner = self.inner.lock().expect("observer mutex poisoned");
        let decay = self.config.decay_per_tick;
        if inner.tick > 0 {
            #[allow(clippy::cast_precision_loss)]
            let prior = f64::from(inner.headers_in_current_section);
            inner.headers_per_section_ema = Some(match inner.headers_per_section_ema {
                Some(prev) => prev * decay + prior * (1.0 - decay),
                None => prior,
            });
        }
        inner.headers_in_current_section = 0;
        inner.tick += 1;
        inner.total_ema = inner.total_ema * decay + 1.0;
    }

    /// Suggested ring size for a new per-connection [`RecentPairs`] hash ring, derived
    /// from the cross-connection EMA on headers-per-section. Clamped to a sensible
    /// `[FLOOR, CEILING]` range. Returns `FLOOR` until the EMA has been primed by at
    /// least one section close.
    ///
    /// Rationale: the per-connection ring is a "have I seen this `(name, value)` pair
    /// recently?" check. Its useful retention window is roughly the typical section
    /// length — a ring shorter than a section can't catch in-section repetition; a ring
    /// much longer than a typical section just costs cache footprint without helping.
    /// The observer has the global view; let it pick.
    ///
    /// [`RecentPairs`]: super::encoder_dynamic_table::RecentPairs
    pub(in crate::headers) fn suggested_ring_size(&self) -> usize {
        const FLOOR: usize = 12;
        const CEILING: usize = 256;
        let inner = self.inner.lock().expect("observer mutex poisoned");
        let target = match inner.headers_per_section_ema {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Some(ema) if ema.is_finite() && ema > 0.0 => ema.round() as usize,
            _ => FLOOR,
        };
        target.clamp(FLOOR, CEILING)
    }

    /// Record one observation of `(name, value)` in the current section. Bumps two
    /// counters in the unified frequency map:
    ///
    /// - The name-only entry `(name, None)` — always, including for sensitive headers. Storing it
    ///   costs nothing if the name has a static-table match (in which case the cost model in
    ///   [`prime`](Self::prime) filters it out anyway), and is the only way to prime hot non-static
    ///   names whose values vary per request (`x-trace-id`, `x-request-id`, custom app headers).
    /// - The full-pair entry `(name, Some(value))` — only when the value is cacheable. See
    ///   [`value_is_uncacheable`] for the skip rules (sensitive headers + Date).
    ///
    /// No per-section dedup is enforced: a pair appearing twice in one field section
    /// (rare — multi-valued headers like `set-cookie` are distinct pairs, but
    /// pathological duplicates would double-count) bumps the count twice.
    pub(in crate::headers) fn record_observation(
        &self,
        name: EntryName<'static>,
        value: FieldLineValue<'static>,
    ) {
        let mut inner = self.inner.lock().expect("observer mutex poisoned");
        // Bump the per-section header count for every observation — feeds
        // `headers_per_section_ema` regardless of cacheability.
        inner.headers_in_current_section = inner.headers_in_current_section.saturating_add(1);

        let tick = inner.tick;
        let decay = self.config.decay_per_tick;
        let cache_value = !value_is_uncacheable(&name);

        // Name-only entry: always tracked.
        let name_entry = inner.entries.entry((name.clone(), None)).or_default();
        name_entry.count = name_entry.effective_count_at(tick, decay) + 1.0;
        name_entry.last_updated_tick = tick;

        // Full-pair entry: only when the value is cacheable.
        if cache_value {
            let pair_entry = inner.entries.entry((name, Some(value))).or_default();
            pair_entry.count = pair_entry.effective_count_at(tick, decay) + 1.0;
            pair_entry.last_updated_tick = tick;
        }

        // Eviction runs once after both insertions; LFU naturally favors high-count
        // entries (the name-only entry tends to dominate within its name family because
        // it sums all per-value counts).
        let over_budget = inner.entries.len() > self.config.max_entries as usize;
        if over_budget {
            evict_lfu(&mut inner, decay);
        }
    }

    /// Whether the observer currently considers `(name, value)` — or the name-only entry
    /// `(name, None)` — hot by the same [`MIN_PRIMING_FRACTION`] threshold that
    /// [`prime`](Self::prime) uses for ranking candidates. Consulted by the encoder's
    /// dup-draining refresh pass to decide whether an oldest-in-table entry is worth
    /// preserving via a Duplicate instruction instead of letting it evict.
    ///
    /// Returns `false` during warm-up (`tick < WARMUP_MIN_TICKS`) or when no observations
    /// exist yet. The HashMap lookup requires an owned key; the clone is one small
    /// allocation on the value side (zero for `Static` / `Borrowed` variants).
    pub(in crate::headers) fn is_hot(
        &self,
        name: &EntryName<'static>,
        value: Option<&FieldLineValue<'static>>,
    ) -> bool {
        let inner = self.inner.lock().expect("observer mutex poisoned");
        if inner.tick < WARMUP_MIN_TICKS || inner.total_ema <= 0.0 {
            return false;
        }
        let min_count = inner.total_ema * MIN_PRIMING_FRACTION;
        let key = (name.clone(), value.cloned());
        inner.entries.get(&key).is_some_and(|stats| {
            stats.effective_count_at(inner.tick, self.config.decay_per_tick) >= min_count
        })
    }

    /// Return priming-insert candidates ranked by expected net byte savings, fitting
    /// under `capacity` bytes (sum of RFC 9204 §3.2.1 entry sizes). Each candidate carries
    /// its EMA count, the observer's total EMA at ranking time, and the computed rank
    /// score — diagnostic fields for research-mode introspection.
    ///
    /// **Cost-aware ranking.** A candidate's score is
    /// `count × savings_per_ref − insert_cost`. Full-pair and name-only candidates are
    /// ranked together; the cost model branches on whether the pair has a static-table
    /// match and whether the value is cached:
    ///
    /// - **Full pair, full static match**: skipped — Indexed Static is already as cheap as Indexed
    ///   Dynamic.
    /// - **Full pair, static name match**: insert is "Insert With Name Reference (T=1)"; per-ref
    ///   savings is the value bytes (Indexed Dynamic vs. Literal With Name Ref).
    /// - **Full pair, no static match**: insert is "Insert With Literal Name"; per-ref savings is
    ///   roughly `name.len() + value.len()`.
    /// - **Name-only, static name match**: skipped — literals can already use the static name ref
    ///   for free.
    /// - **Name-only, no static name match**: insert is "Insert With Literal Name" carrying empty
    ///   value; per-ref savings is `name.len()` (Literal With Dynamic Name Ref vs Literal With
    ///   Literal Name).
    ///
    /// Candidates whose net score is non-positive are dropped.
    ///
    /// Empty when there's insufficient data (warm-up), no eligible pairs, or capacity is
    /// zero.
    pub(in crate::headers) fn prime(&self, capacity: u32) -> Vec<PrimingCandidate> {
        let inner = self.inner.lock().expect("observer mutex poisoned");
        if inner.tick < WARMUP_MIN_TICKS {
            return Vec::new();
        }
        let tick = inner.tick;
        let decay = self.config.decay_per_tick;
        let total_ema = inner.total_ema;
        let min_count = total_ema * MIN_PRIMING_FRACTION;

        let mut candidates: Vec<_> = inner
            .entries
            .iter()
            .filter_map(|((name, value), stats)| {
                let count = stats.effective_count_at(tick, decay);
                if count < min_count {
                    return None;
                }
                let CostModel {
                    insert_cost,
                    savings_per_ref,
                } = CostModel::estimate(name, value.as_ref())?;
                let gross_savings = count * f64::from(savings_per_ref);
                let net_score = gross_savings - f64::from(insert_cost);
                if net_score <= 0.0 {
                    return None;
                }
                let value_len = value.as_ref().map_or(0, |v| v.len());
                let entry_size = ENTRY_OVERHEAD
                    .saturating_add(u32::try_from(name.len()).unwrap_or(u32::MAX))
                    .saturating_add(u32::try_from(value_len).unwrap_or(u32::MAX));
                Some(RankedCandidate {
                    name: name.clone(),
                    value: value.clone(),
                    entry_size,
                    count,
                    score: net_score,
                })
            })
            .collect();

        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut out = Vec::new();
        let mut used: u32 = 0;
        for c in candidates {
            match used.checked_add(c.entry_size) {
                Some(next) if next <= capacity => {
                    used = next;
                    out.push(PrimingCandidate {
                        name: c.name,
                        value: c.value,
                        count: c.count,
                        total_ema,
                        score: c.score,
                    });
                }
                _ => break,
            }
        }
        out
    }
}

/// Approximate cost model for one priming candidate. See [`HeaderObserver::prime`] for
/// the rationale behind each branch. Numbers are wire-byte estimates that ignore varint
/// width and Huffman compression — close enough for ranking, and a miss in either
/// direction just shifts the priming threshold by a byte or two.
struct CostModel {
    /// Estimated encoder-stream bytes to insert the entry.
    insert_cost: u32,
    /// Estimated bytes saved per reference: (no-priming encoding cost) − (Indexed
    /// Dynamic encoding cost ≈ 2 bytes).
    savings_per_ref: u32,
}

impl CostModel {
    /// Estimate the cost/savings of priming `(name, value)` (when `value` is `Some`) or
    /// the name-only entry `(name, "")` (when `value` is `None`). Returns `None` when
    /// priming is dominated by a cheaper alternative the encoder will already pick:
    ///
    /// - Full pair with a full static-table match — Indexed Static is already as cheap.
    /// - Name-only with a static name-table match — literals can use the static name ref for free.
    fn estimate(name: &EntryName<'_>, value: Option<&FieldLineValue<'_>>) -> Option<Self> {
        let name_len = u32::try_from(name.len()).unwrap_or(u32::MAX);
        let value_bytes = value.map(FieldLineValue::as_bytes);
        let lookup = static_table::static_table_lookup(name, value_bytes);

        match (value, lookup) {
            // Full pair with full static match: Indexed Static is already 1-2 bytes,
            // matching Indexed Dynamic. Skip.
            (Some(_), static_table::StaticLookup::FullMatch(_)) => None,

            // Full pair, static name match: insert is "Insert With Name Reference (T=1)".
            // Per-ref savings is the value bytes (Literal With Name Ref T=1 vs Indexed
            // Dynamic).
            (Some(v), static_table::StaticLookup::NameMatch(_)) => {
                let value_len = u32::try_from(v.len()).unwrap_or(u32::MAX);
                Some(Self {
                    insert_cost: value_len.saturating_add(2),
                    savings_per_ref: value_len,
                })
            }

            // Full pair, no static match: insert is "Insert With Literal Name". Per-ref
            // savings is roughly name+value bytes (Literal With Literal Name vs Indexed
            // Dynamic).
            (Some(v), static_table::StaticLookup::NoMatch) => {
                let value_len = u32::try_from(v.len()).unwrap_or(u32::MAX);
                Some(Self {
                    insert_cost: name_len.saturating_add(value_len).saturating_add(3),
                    savings_per_ref: name_len.saturating_add(value_len).saturating_add(1),
                })
            }

            // Name-only with any static name match (full or partial): literals can use
            // the static name ref for free. Skip.
            (
                None,
                static_table::StaticLookup::FullMatch(_) | static_table::StaticLookup::NameMatch(_),
            ) => None,

            // Name-only, no static match: insert is "Insert With Literal Name" carrying
            // empty value (~3 prefix + name). Per-ref savings is `name.len()` (Literal
            // With Dynamic Name Ref vs Literal With Literal Name).
            (None, static_table::StaticLookup::NoMatch) => Some(Self {
                insert_cost: name_len.saturating_add(3),
                savings_per_ref: name_len,
            }),
        }
    }
}

/// Priming-insert candidate returned by [`HeaderObserver::prime`]. Carries research-mode
/// diagnostic fields (`count`, `total_ema`, `score`) alongside the pair itself so callers
/// can surface priming decisions in logs without re-querying the observer.
///
/// `value` is `None` for a name-only candidate — the encoder primes it as a
/// `(name, "")` dynamic-table entry so future literals can use a `LiteralDynamicNameRef`
/// to save the name bytes.
#[derive(Debug)]
pub(in crate::headers) struct PrimingCandidate {
    pub(in crate::headers) name: EntryName<'static>,
    pub(in crate::headers) value: Option<FieldLineValue<'static>>,
    /// EMA-decayed count of connections that observed this entry, at ranking time.
    pub(in crate::headers) count: f64,
    /// Observer's `total_ema` at ranking time. `count / total_ema` is the fraction of
    /// recent sections that observed this entry.
    pub(in crate::headers) total_ema: f64,
    /// Cost-aware rank score used to order candidates:
    /// `count × savings_per_ref − insert_cost`.
    pub(in crate::headers) score: f64,
}

/// Internal ranking record used only within [`HeaderObserver::prime`]. Holds the
/// `entry_size` needed for capacity bin-packing, which [`PrimingCandidate`] does not
/// need to expose to callers.
struct RankedCandidate {
    name: EntryName<'static>,
    value: Option<FieldLineValue<'static>>,
    entry_size: u32,
    count: f64,
    score: f64,
}

/// True when the *value* of this header should not be cached by the observer. Names that
/// are sensitive ([`EntryName::has_uncacheable_value`]) and `date` (rolls over every
/// second) qualify. The name itself is still tracked — name-only priming is safe and
/// useful even for these.
fn value_is_uncacheable(name: &EntryName<'_>) -> bool {
    if name.has_uncacheable_value() {
        return true;
    }
    matches!(name, EntryName::Known(KnownHeaderName::Date))
}

/// Drop the least-frequently-used entry (by current effective count) to stay within the
/// configured max. Called when an insert overflows the budget — one eviction per overflow
/// keeps the map size bounded without a bulk pass.
fn evict_lfu(inner: &mut ObserverInner, decay: f64) {
    let tick = inner.tick;
    let victim = inner
        .entries
        .iter()
        .min_by(|(_, a), (_, b)| {
            a.effective_count_at(tick, decay)
                .partial_cmp(&b.effective_count_at(tick, decay))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(k, _)| k.clone());
    if let Some(key) = victim {
        inner.entries.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KnownHeaderName;

    fn name(known: KnownHeaderName) -> EntryName<'static> {
        EntryName::Known(known)
    }

    fn value(bytes: &'static [u8]) -> FieldLineValue<'static> {
        FieldLineValue::Static(bytes)
    }

    fn observer_with(half_life: u32, max_entries: u32) -> HeaderObserver {
        let mut cfg = HttpConfig::DEFAULT;
        cfg.h3_qpack_header_observer_half_life_sections = half_life;
        cfg.h3_qpack_header_observer_max_entries = max_entries;
        HeaderObserver::from_http_config(&cfg)
    }

    /// Drive `n` sections that each observe the same single pair.
    fn observe_n_same(
        observer: &HeaderObserver,
        n: usize,
        n_pair: (EntryName<'static>, FieldLineValue<'static>),
    ) {
        for _ in 0..n {
            observer.record_section_start();
            observer.record_observation(n_pair.0.clone(), n_pair.1.clone());
        }
    }

    // warmup_suppresses_priming removed: WARMUP_MIN_TICKS is currently 0 (research-mode
    // override, see the const's doc comment). Re-add coverage when the quality filter is
    // restored.

    #[test]
    fn prime_emits_ubiquitous_pair_after_warmup() {
        // (server, "trillium"): server is a static name (with empty value), so the
        // full-pair candidate gets value-bytes savings; the name-only candidate is
        // skipped by the cost model because the static name ref is already free.
        let observer = observer_with(10_000, 1_000);
        observe_n_same(
            &observer,
            500,
            (name(KnownHeaderName::Server), value(b"trillium")),
        );
        let primed = observer.prime(4096);
        assert_eq!(primed.len(), 1, "expected 1 candidate, got {primed:?}");
        assert_eq!(primed[0].name, name(KnownHeaderName::Server));
        assert_eq!(primed[0].value, Some(value(b"trillium")));
    }

    #[test]
    fn prime_rejects_pair_below_threshold() {
        let observer = observer_with(10_000, 1_000);
        // 500 sections; the pair appears in 100 of them = 20% < 30% threshold.
        for i in 0..500 {
            observer.record_section_start();
            if i < 100 {
                observer.record_observation(name(KnownHeaderName::Server), value(b"trillium"));
            }
        }
        assert!(observer.prime(4096).is_empty());
    }

    #[test]
    fn non_static_uncacheable_name_primes_name_only() {
        // `proxy-authorization` has uncacheable values (per-user secrets) AND isn't in
        // the static name table — both conditions for name-only priming to do useful
        // work. Priming `(proxy-authorization, "")` lets future literals reference it
        // via LiteralDynamicNameRef and save the name bytes, without ever caching a
        // value. The common sensitive headers (Authorization, Cookie, SetCookie) are
        // all in the static table and so get filtered by the cost model — name-only
        // priming gives them no benefit.
        let observer = observer_with(10_000, 1_000);
        observe_n_same(
            &observer,
            500,
            (
                name(KnownHeaderName::ProxyAuthorization),
                value(b"Basic abc123"),
            ),
        );
        let primed = observer.prime(4096);
        assert_eq!(primed.len(), 1, "expected 1 candidate, got {primed:?}");
        assert_eq!(primed[0].name, name(KnownHeaderName::ProxyAuthorization));
        assert_eq!(primed[0].value, None);
    }

    #[test]
    fn date_is_tracked_full_pair_skipped() {
        // `date` is a static name match, so name-only priming is also skipped (the
        // static ref is already free). Full-pair tracking is suppressed because the
        // value rolls over every second. Net: nothing to prime, but for a different
        // reason than the old behavior.
        let observer = observer_with(10_000, 1_000);
        observe_n_same(
            &observer,
            500,
            (
                name(KnownHeaderName::Date),
                value(b"Sun, 20 Apr 2026 12:00:00 GMT"),
            ),
        );
        assert!(observer.prime(4096).is_empty());
    }

    #[test]
    fn prime_respects_capacity_and_ranks_by_savings() {
        // Two pairs, both ubiquitous, but one has a longer value (bigger per-use
        // savings). Capacity only fits one. Both names are static — name-only entries
        // are skipped by the cost model — so we only consider full-pair candidates.
        let observer = observer_with(10_000, 1_000);
        let big = (
            name(KnownHeaderName::ContentType),
            value(b"application/json; charset=utf-8"),
        );
        let small = (name(KnownHeaderName::ContentLength), value(b"12"));
        for _ in 0..500 {
            observer.record_section_start();
            observer.record_observation(big.0.clone(), big.1.clone());
            observer.record_observation(small.0.clone(), small.1.clone());
        }
        // Entry sizes (RFC 9204 §3.2.1: 32 + name.len() + value.len()):
        //   big:   32 + 12 ("content-type")   + 31 ("application/json; charset=utf-8") = 75
        //   small: 32 + 14 ("content-length") +  2 ("12")                              = 48
        // Capacity 75 fits only the big one (small needs 48 more and overflows).
        let primed = observer.prime(75);
        assert_eq!(primed.len(), 1);
        assert_eq!(primed[0].name, big.0);
        assert_eq!(primed[0].value, Some(big.1));
    }

    #[test]
    fn ema_decays_stale_entry_below_threshold() {
        let observer = observer_with(50, 1_000);
        let stale = (name(KnownHeaderName::Server), value(b"old-server"));
        let fresh = (name(KnownHeaderName::ContentType), value(b"text/html"));

        // Observe `stale` for the first 200 sections.
        for _ in 0..200 {
            observer.record_section_start();
            observer.record_observation(stale.0.clone(), stale.1.clone());
        }
        // Then stop observing it, and observe `fresh` for the next 1000 sections.
        // Half-life 50 sections => after 1000 sections `stale` has decayed by
        // ~2^-20, effectively zero.
        for _ in 0..1000 {
            observer.record_section_start();
            observer.record_observation(fresh.0.clone(), fresh.1.clone());
        }
        let primed = observer.prime(4096);
        assert!(
            primed
                .iter()
                .any(|c| c.name == fresh.0 && c.value.as_ref() == Some(&fresh.1)),
            "fresh entry should prime; got {primed:?}"
        );
        assert!(
            !primed
                .iter()
                .any(|c| c.name == stale.0 && c.value.as_ref() == Some(&stale.1)),
            "stale entry should have decayed below threshold; got {primed:?}"
        );
    }

    #[test]
    fn lfu_eviction_keeps_most_common_entries() {
        // Each observation creates up to 2 entries (full-pair + name-only) so 3 names
        // × 2 = 6 logical entries fight for 4 slots. LFU evicts the lowest counts; the
        // least-frequent name (etag) loses both its entries.
        let observer = observer_with(10_000, 4);
        let most = (name(KnownHeaderName::Server), value(b"a"));
        let mid = (name(KnownHeaderName::ContentType), value(b"b"));
        let least = (name(KnownHeaderName::Etag), value(b"c"));

        for i in 0..500 {
            observer.record_section_start();
            observer.record_observation(most.0.clone(), most.1.clone());
            if i % 2 == 0 {
                observer.record_observation(mid.0.clone(), mid.1.clone());
            }
            if i % 10 == 0 {
                observer.record_observation(least.0.clone(), least.1.clone());
            }
        }
        // All three names are static so name-only candidates are filtered by the cost
        // model. Asserting on the surviving full-pair entries.
        let primed = observer.prime(4096);
        assert!(
            primed
                .iter()
                .any(|c| c.name == most.0 && c.value.as_ref() == Some(&most.1))
        );
        assert!(
            primed
                .iter()
                .any(|c| c.name == mid.0 && c.value.as_ref() == Some(&mid.1))
        );
        assert!(
            !primed
                .iter()
                .any(|c| c.name == least.0 && c.value.as_ref() == Some(&least.1)),
            "least-frequent pair should have been evicted"
        );
    }

    #[test]
    fn high_cardinality_stable_name_primes_name_only() {
        // `x-trace-id` is non-static, with values that vary per request (no value
        // crosses the 30% fraction threshold) but the name itself is hot. Expectation:
        // prime emits a single name-only candidate `(x-trace-id, None)`.
        let observer = observer_with(10_000, 1_000);
        let trace_name: EntryName<'static> = EntryName::try_from(b"x-trace-id".to_vec()).unwrap();
        for i in 0..500 {
            observer.record_section_start();
            // Distinct value per section — no full-pair clears the threshold.
            let value_bytes = format!("trace-{i:04}").into_bytes();
            observer.record_observation(trace_name.clone(), FieldLineValue::Owned(value_bytes));
        }
        let primed = observer.prime(4096);
        let name_only_match = primed
            .iter()
            .find(|c| c.name == trace_name && c.value.is_none());
        assert!(
            name_only_match.is_some(),
            "expected a name-only candidate for x-trace-id; got {primed:?}",
        );
        // No full-pair candidate should sneak in (no single value cleared the threshold).
        assert!(
            !primed
                .iter()
                .any(|c| c.name == trace_name && c.value.is_some()),
            "no per-value full-pair candidate should be present; got {primed:?}",
        );
    }

    #[test]
    fn section_tick_advances_total() {
        let observer = observer_with(10_000, 1_000);
        for _ in 0..250 {
            observer.record_section_start();
        }
        let inner = observer.inner.lock().unwrap();
        assert_eq!(inner.tick, 250);
        // EMA asymptote is 1/(1-decay). At half_life 10_000 and 250 ticks the accumulator
        // is meaningfully below tick count — approximately (1 - 0.5^(250/10000)) / (1 - decay)
        // ≈ 248.5. Bounded above by tick.
        assert!(inner.total_ema < 250.0);
        assert!(inner.total_ema > 240.0, "total_ema = {}", inner.total_ema);
    }

    #[test]
    fn suggested_ring_size_returns_floor_before_first_section() {
        let observer = observer_with(10_000, 1_000);
        // No sections recorded yet — EMA unprimed.
        assert_eq!(observer.suggested_ring_size(), 12);
    }

    #[test]
    fn suggested_ring_size_tracks_headers_per_section_ema() {
        let observer = observer_with(10, 1_000);
        let pair = (name(KnownHeaderName::Server), value(b"trillium"));
        // Drive 200 sections of 25 headers each. Half-life of 10 sections converges fast.
        for _ in 0..200 {
            observer.record_section_start();
            for _ in 0..25 {
                observer.record_observation(pair.0.clone(), pair.1.clone());
            }
        }
        // One more section start to fold the most recent count into the EMA.
        observer.record_section_start();
        let suggested = observer.suggested_ring_size();
        assert!(
            (24..=26).contains(&suggested),
            "suggested ring size {suggested} should be ~25",
        );
    }

    #[test]
    fn suggested_ring_size_clamped_to_ceiling() {
        let observer = observer_with(10, 1_000);
        let pair = (name(KnownHeaderName::Server), value(b"trillium"));
        // 1000 headers/section is well above the 256 ceiling.
        for _ in 0..50 {
            observer.record_section_start();
            for _ in 0..1000 {
                observer.record_observation(pair.0.clone(), pair.1.clone());
            }
        }
        observer.record_section_start();
        assert_eq!(observer.suggested_ring_size(), 256);
    }
}
