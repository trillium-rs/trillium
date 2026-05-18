//! Cross-connection header observer.
//!
//! Tracks the *set* of `(name, value)` pairs (and the set of names) the application
//! has emitted across the lifetime of this listener, so each new connection's dynamic
//! table can be pre-warmed with literals that the encoder is likely to emit again.
//!
//! Protocol-agnostic: the observation pool is shared across HPACK and QPACK encoders
//! on the same listener (HTTP/2 and HTTP/3 see the same application headers, so an
//! observation from either feeds both). The cost model branches on
//! [`HeaderCompression`] at consultation time.
//!
//! ## Type-narrowed exact-identity design
//!
//! Cross-connection priming is restricted to pairs whose name has a [`NameKey`]
//! representation — `Known(K)`, `Pseudo(P)`, or `UnknownStatic(&'static str)`. All
//! three are program-controlled by construction:
//!
//! - `Known(K)` and `Pseudo(P)` are sealed enums populated from compile-time constants in
//!   application source.
//! - `UnknownStatic(&'static str)` is the result of routing a `&'static str` literal through the
//!   lowercase interner ([`super::unknown_header_name`]). The interner only takes `&'static str`
//!   inputs and only adds entries for literals that already lived in static memory.
//!
//! Pair tracking additionally requires the value be `FieldLineValue::Static`
//! (`&'static [u8]`). Borrowed-non-static and Owned values are not paired; only the
//! name dimension is recorded for them.
//!
//! This makes the observer safe against value-exfiltration via reflected request
//! data (a hot reflected name cannot promote into priming, because `Unknown` is
//! excluded) AND cheap on the hot path (no hashing, no allocation, no mutex per
//! header line — only at connection close).
//!
//! ## Storage shape
//!
//! Just two `HashSet`s. No counts, no epochs, no decay. Once a pair is observed in
//! any connection, it stays in the priming set for the lifetime of the listener.
//! The set is bounded by source-code-reachable literals (typically <100 entries
//! server-wide), so unbounded growth isn't a real concern.
//!
//! Priming ranks by `CostModel::savings_per_ref` (descending) and bin-packs under
//! the negotiated capacity. The cost model filters candidates that the encoder
//! would already emit cheaply (full static-table match, etc.); after that, longer
//! values prime first because they save more bytes per reference.
//!
//! Role isolation: each hop-and-direction pair gets its own observer (see
//! `HttpContext::__isolate_qpack_observer`).

use crate::{
    KnownHeaderName,
    headers::{
        entry_name::{EntryName, PseudoHeaderName},
        field_section::FieldLineValue,
        static_hit::StaticHit,
    },
};
use hashbrown::HashSet;
use smallvec::SmallVec;
use std::{
    fmt::{self, Debug},
    sync::Mutex,
};

/// Per-entry overhead in the dynamic table (entry size = overhead + name bytes +
/// value bytes). Identical for HPACK and QPACK.
const ENTRY_OVERHEAD: u32 = 32;

/// Which header-compression scheme to cost a priming candidate against. Selects
/// per-protocol wire-byte constants in [`CostModel::estimate`]; the observation pool
/// itself is protocol-agnostic.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[allow(
    dead_code,
    reason = "Hpack arm currently unused; the cost model already supports it"
)]
pub(crate) enum HeaderCompression {
    /// HPACK — HTTP/2 header compression. Inserts inline in HEADERS blocks.
    Hpack,
    /// QPACK — HTTP/3 header compression. Inserts on the encoder stream.
    Qpack,
}

/// Stable, content-equal key for a header name. All three variants are `Copy` and
/// program-controlled by construction.
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub(in crate::headers) enum NameKey {
    Known(KnownHeaderName),
    Pseudo(PseudoHeaderName),
    UnknownStatic(&'static str),
}

impl Debug for NameKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known(arg0) => write!(f, "{arg0}"),
            Self::Pseudo(arg0) => write!(f, "{arg0}"),
            Self::UnknownStatic(arg0) => write!(f, "{arg0:?}"),
        }
    }
}

impl NameKey {
    /// Reconstitute the corresponding `EntryName<'static>`.
    fn into_entry_name(self) -> EntryName<'static> {
        match self {
            Self::Known(k) => EntryName::Known(k),
            Self::Pseudo(p) => EntryName::Pseudo(p),
            Self::UnknownStatic(s) => EntryName::UnknownStatic(s),
        }
    }
}

/// Per-listener tracker of header-name and `(name, value)` sets, consulted when
/// priming a new connection's dynamic table.
#[derive(Debug, Default)]
pub(crate) struct HeaderObserver {
    inner: Mutex<ObserverInner>,
}

#[derive(Default)]
struct ObserverInner {
    /// All `(name, &'static [u8])` pairs ever observed across connections.
    seen_pairs: HashSet<(NameKey, &'static [u8])>,
    /// All names ever observed across connections.
    seen_names: HashSet<NameKey>,
}

impl Debug for ObserverInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ObserverInner")
            .field(
                "seen_pairs",
                &fmt::from_fn(|f| {
                    let mut map = f.debug_map();

                    for (name, value) in &self.seen_pairs {
                        map.entry(&name, &format_args!("{}", String::from_utf8_lossy(value)));
                    }

                    map.finish()?;
                    Ok(())
                }),
            )
            .field("seen_names", &self.seen_names)
            .finish()
    }
}

impl HeaderObserver {
    /// Fold a connection's accumulator into the shared sets. Called exactly once
    /// per connection at encoder shutdown. The only mutating path on the shared
    /// observer; no contention with the encode hot path.
    pub(in crate::headers) fn fold_connection(&self, accum: &ConnectionAccumulator) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let pairs_before = inner.seen_pairs.len();
        let names_before = inner.seen_names.len();
        for &pair in &accum.seen_pairs {
            inner.seen_pairs.insert(pair);
        }
        for &name in &accum.seen_names {
            inner.seen_names.insert(name);
        }
        let pairs_after = inner.seen_pairs.len();
        let names_after = inner.seen_names.len();
        log::debug!(
            target: "qpack_metrics",
            "observer fold: contributed pairs={} names={} | shared seen_pairs {pairs_before}->{pairs_after} \
             seen_names {names_before}->{names_after}",
            accum.seen_pairs.len(),
            accum.seen_names.len(),
        );
    }

    /// True iff `name` (with optional `value`) has ever been observed across any
    /// connection.
    ///
    /// For `value = Some(FieldLineValue::Static(s))`, looks up the exact pair.
    /// For other value variants (or `None`), falls back to the name-only set —
    /// runtime-allocated values aren't paired but their name dimension is.
    pub(in crate::headers) fn is_hot(
        &self,
        name: &EntryName<'_>,
        value: Option<&FieldLineValue<'_>>,
    ) -> bool {
        let Some(key) = name.name_key() else {
            return false;
        };
        let Ok(inner) = self.inner.lock() else {
            return false;
        };
        match value {
            Some(FieldLineValue::Static(s)) => inner.seen_pairs.contains(&(key, *s)),
            _ => inner.seen_names.contains(&key),
        }
    }

    /// Return priming-insert candidates ranked by `CostModel::savings_per_ref`
    /// (descending), fitting under `capacity` bytes. Each candidate is a pair or
    /// name-only entry the encoder would otherwise spend wire bytes on if literal-
    /// emitted. The `compression` parameter selects the wire-byte cost model.
    ///
    /// Empty when no observations have happened yet, no candidates pass the cost
    /// model, or capacity is zero.
    pub(in crate::headers) fn prime(
        &self,
        capacity: u32,
        compression: HeaderCompression,
    ) -> Vec<PrimingCandidate> {
        if capacity == 0 {
            return Vec::new();
        }
        let Ok(inner) = self.inner.lock() else {
            return Vec::new();
        };

        let observed_pairs = inner.seen_pairs.len();
        let observed_names = inner.seen_names.len();

        let mut ranked: Vec<RankedCandidate> = Vec::new();
        for &(key, s) in &inner.seen_pairs {
            let name = key.into_entry_name();
            let value = FieldLineValue::Static(s);
            push_candidate(&mut ranked, name, Some(value), compression);
        }
        for &key in &inner.seen_names {
            let name = key.into_entry_name();
            push_candidate(&mut ranked, name, None, compression);
        }
        let ranked_total = ranked.len();

        // Rank by per-reference savings (descending); on ties prefer the smaller
        // entry so we pack more candidates into the budget.
        ranked.sort_by(|a, b| {
            b.savings_per_ref
                .cmp(&a.savings_per_ref)
                .then_with(|| a.entry_size.cmp(&b.entry_size))
        });

        let mut out: Vec<PrimingCandidate> = Vec::new();
        let mut used: u32 = 0;
        let mut dropped_no_room = 0usize;
        for c in ranked {
            match used.checked_add(c.entry_size) {
                Some(next) if next <= capacity => {
                    used = next;
                    if log::log_enabled!(target: "qpack_metrics", log::Level::Trace) {
                        log::trace!(
                            target: "qpack_metrics",
                            "  primed [{idx}]: savings/ref={savings} entry_size={size} name={name:?} value={value}",
                            idx = out.len(),
                            savings = c.savings_per_ref,
                            size = c.entry_size,
                            name = c.name,
                            value = match &c.value {
                                Some(v) => format!("{:?}", String::from_utf8_lossy(v.as_bytes())),
                                None => "<name-only>".to_string(),
                            },
                        );
                    }
                    out.push(PrimingCandidate {
                        name: c.name,
                        value: c.value,
                    });
                }
                _ => {
                    dropped_no_room += 1;
                }
            }
        }
        log::debug!(
            target: "qpack_metrics",
            "observer prime(capacity={capacity}, {compression:?}): observed pairs={observed_pairs} names={observed_names} \
             cost-passing={ranked_total} packed={} dropped_no_room={dropped_no_room} bytes_used={used}/{capacity}",
            out.len(),
        );
        out
    }
}

fn push_candidate(
    ranked: &mut Vec<RankedCandidate>,
    name: EntryName<'static>,
    value: Option<FieldLineValue<'static>>,
    compression: HeaderCompression,
) {
    let Some(model) = CostModel::estimate(compression, &name, value.as_ref()) else {
        return;
    };
    let value_len = value.as_ref().map_or(0, |v| v.as_bytes().len());
    let entry_size = ENTRY_OVERHEAD
        .saturating_add(u32::try_from(name.len()).unwrap_or(u32::MAX))
        .saturating_add(u32::try_from(value_len).unwrap_or(u32::MAX));
    ranked.push(RankedCandidate {
        name,
        value,
        entry_size,
        savings_per_ref: model.savings_per_ref,
    });
}

/// Per-connection observation accumulator. Lives inline on `TableState` (already
/// lock-protected during planning), so the hot path adds no mutex traffic. Folded
/// into the shared observer in a single mutex acquisition at connection close.
#[derive(Default)]
pub(crate) struct ConnectionAccumulator {
    /// Distinct `(NameKey, &'static [u8])` pairs observed this connection, with
    /// names that have not gone high-cardinality. Linear-scan dedup; typical
    /// `N <= ~20` distinct program-emitted pairs, so beats hashing.
    seen_pairs: SmallVec<[(NameKey, &'static [u8]); 16]>,
    /// Names where two distinct Static values have been observed this connection.
    /// Once a name is in this set, subsequent Static observations skip
    /// `seen_pairs` (no point tracking pairs whose values vary). The original
    /// entry is removed from `seen_pairs` when the high-card transition happens,
    /// so name-only priming wins over a single-shot pair.
    high_card_names: SmallVec<[NameKey; 4]>,
    /// Names observed at least once this connection (any value variant). Folded
    /// into `seen_names` at connection close.
    seen_names: SmallVec<[NameKey; 32]>,
}

impl Debug for ConnectionAccumulator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionAccumulator")
            .field(
                "seen_pairs",
                &fmt::from_fn(|f| {
                    let mut f = f.debug_map();
                    for (name, value) in &self.seen_pairs {
                        f.entry(name, &format_args!("{}", String::from_utf8_lossy(value)));
                    }
                    f.finish()
                }),
            )
            .field("high_card_names", &self.high_card_names)
            .field("seen_names", &self.seen_names)
            .finish()
    }
}

impl ConnectionAccumulator {
    /// Hot path. One call per emitted header line. Names without a [`NameKey`]
    /// representation (i.e., `EntryName::Unknown` — borrowed-non-static or
    /// Owned) are skipped entirely; the rest mark their name-set bit. Pair
    /// tracking additionally requires `Static` value AND a non-uncacheable name.
    pub(in crate::headers) fn observe(&mut self, name: &EntryName<'_>, value: &FieldLineValue<'_>) {
        let Some(key) = name.name_key() else {
            return;
        };
        let static_value = if name.has_uncacheable_value() {
            None
        } else {
            match value {
                FieldLineValue::Static(s) => Some(*s),
                _ => None,
            }
        };
        self.record(key, static_value);
    }

    /// Pre-extracted form of [`observe`](Self::observe) for callers that already
    /// have the `(NameKey, static_value)` pair in hand.
    ///
    /// `static_value` is `Some(s)` only for non-uncacheable names with
    /// `FieldLineValue::Static` values — exactly the cases [`observe`] would
    /// have considered for full-pair tracking. `None` covers both the
    /// uncacheable-name and non-Static-value cases.
    pub(in crate::headers) fn record(&mut self, key: NameKey, static_value: Option<&'static [u8]>) {
        if !self.seen_names.contains(&key) {
            self.seen_names.push(key);
        }

        let Some(s) = static_value else {
            return;
        };
        if self.high_card_names.contains(&key) {
            return;
        }

        let mut same_pos: Option<usize> = None;
        let mut diff_pos: Option<usize> = None;
        for (i, (kk, ss)) in self.seen_pairs.iter().enumerate() {
            if *kk != key {
                continue;
            }
            if *ss == s {
                same_pos = Some(i);
                break;
            }
            diff_pos = Some(i);
        }

        match (same_pos, diff_pos) {
            (Some(_), _) => {} // already tracked
            (None, Some(i)) => {
                // Second distinct Static value for this name → high-card. Drop
                // the single-pair entry so name-only priming takes over.
                self.seen_pairs.swap_remove(i);
                self.high_card_names.push(key);
            }
            (None, None) => {
                self.seen_pairs.push((key, s));
            }
        }
    }
}

/// Approximate cost model for one priming candidate. Wire-byte estimates that
/// ignore varint width and Huffman compression — close enough for ranking, and a
/// miss in either direction just shifts the priming threshold by a byte or two.
struct CostModel {
    /// Estimated bytes saved per reference: (no-priming encoding cost) − (indexed
    /// reference encoding cost). The indexed cost differs per protocol (QPACK
    /// indexed-dynamic ≈ 2 bytes; HPACK indexed ≈ 1 byte at typical dyn indices),
    /// hence the [`HeaderCompression`] dispatch in [`Self::estimate`].
    savings_per_ref: u32,
}

impl CostModel {
    /// Estimate the savings of priming `(name, value)` (full-pair when `value` is
    /// `Some`) or the name-only entry `(name, "")` (when `value` is `None`).
    /// Returns `None` when priming is dominated by a cheaper alternative the
    /// encoder would already pick:
    ///
    /// - Full pair with a full static-table match — Indexed Static is already as cheap.
    /// - Name-only with a static name-table match — literals can use the static name ref for free.
    ///
    /// The `compression` parameter selects per-protocol wire-byte constants. The
    /// `(NoMatch)` arms use slightly different overhead numbers because HPACK's
    /// Indexed form is 1 byte at typical dynamic indices while QPACK's
    /// `IndexedDynamic` is ~2 bytes; the cost-model output is rough enough that the
    /// difference only matters at the ranking margins.
    #[allow(
        clippy::match_same_arms,
        reason = "arms differ semantically (None vs StaticHit::Full/Name) and are kept separate \
                  for clarity"
    )]
    fn estimate(
        compression: HeaderCompression,
        name: &EntryName<'_>,
        value: Option<&FieldLineValue<'_>>,
    ) -> Option<Self> {
        let name_len = u32::try_from(name.len()).unwrap_or(u32::MAX);
        let value_bytes = value.map(FieldLineValue::as_bytes);
        let lookup = static_lookup(compression, name, value_bytes);

        match (value, lookup) {
            (Some(_), StaticHit::Full(_)) => None,

            (Some(v), StaticHit::Name(_)) => {
                let value_len = u32::try_from(v.len()).unwrap_or(u32::MAX);
                Some(Self {
                    savings_per_ref: value_len,
                })
            }

            (Some(v), StaticHit::None) => {
                let value_len = u32::try_from(v.len()).unwrap_or(u32::MAX);
                let overhead = match compression {
                    HeaderCompression::Qpack => 1,
                    HeaderCompression::Hpack => 2,
                };
                Some(Self {
                    savings_per_ref: name_len.saturating_add(value_len).saturating_add(overhead),
                })
            }

            (None, StaticHit::Full(_) | StaticHit::Name(_)) => None,

            (None, StaticHit::None) => Some(Self {
                savings_per_ref: name_len,
            }),
        }
    }
}

/// Run the per-protocol static-table lookup. HPACK's lookup signature takes a
/// non-optional `&[u8]`; for name-only candidates (`value = None`) we pass `b""` so
/// shared-`""`-value entries surface as `Name`, not `Full`.
fn static_lookup(
    compression: HeaderCompression,
    name: &EntryName<'_>,
    value: Option<&[u8]>,
) -> StaticHit {
    match compression {
        HeaderCompression::Qpack => {
            crate::headers::qpack::static_table::static_table_lookup(name, value)
        }
        HeaderCompression::Hpack => {
            crate::headers::hpack::static_table::static_table_lookup(name, value.unwrap_or(b""))
        }
    }
}

/// Priming-insert candidate returned by [`HeaderObserver::prime`]. `value` is
/// `None` for a name-only candidate — the encoder primes it as a `(name, "")`
/// dynamic-table entry so future literals can use a name-reference form to save
/// the name bytes.
#[derive(Debug)]
pub(in crate::headers) struct PrimingCandidate {
    pub(in crate::headers) name: EntryName<'static>,
    pub(in crate::headers) value: Option<FieldLineValue<'static>>,
}

/// Internal ranking record used only within [`HeaderObserver::prime`]. Holds the
/// `entry_size` needed for capacity bin-packing and the `savings_per_ref` used
/// for ranking, neither of which [`PrimingCandidate`] needs to expose.
struct RankedCandidate {
    name: EntryName<'static>,
    value: Option<FieldLineValue<'static>>,
    entry_size: u32,
    savings_per_ref: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(known: KnownHeaderName) -> EntryName<'static> {
        EntryName::Known(known)
    }

    fn value(bytes: &'static [u8]) -> FieldLineValue<'static> {
        FieldLineValue::Static(bytes)
    }

    fn observe_once(
        observer: &HeaderObserver,
        pairs: &[(EntryName<'static>, FieldLineValue<'static>)],
    ) {
        let mut accum = ConnectionAccumulator::default();
        for (n, v) in pairs {
            accum.observe(n, v);
        }
        observer.fold_connection(&accum);
    }

    #[test]
    fn prime_emits_observed_pair_after_one_connection() {
        // Single observation is enough — there is no warmup gate.
        let observer = HeaderObserver::default();
        observe_once(
            &observer,
            &[(name(KnownHeaderName::Server), value(b"trillium"))],
        );
        let primed = observer.prime(4096, HeaderCompression::Qpack);
        assert_eq!(primed.len(), 1, "expected 1 candidate, got {primed:?}");
        assert_eq!(primed[0].name, name(KnownHeaderName::Server));
        assert_eq!(primed[0].value, Some(value(b"trillium")));
    }

    #[test]
    fn prime_skips_full_static_match() {
        // (:status, 200) is a full static match — not worth priming.
        let observer = HeaderObserver::default();
        observe_once(
            &observer,
            &[(EntryName::Pseudo(PseudoHeaderName::Status), value(b"200"))],
        );
        let primed = observer.prime(4096, HeaderCompression::Qpack);
        assert!(
            !primed.iter().any(
                |c| matches!(c.name, EntryName::Pseudo(PseudoHeaderName::Status))
                    && c.value.is_some()
            ),
            "(:status, 200) should not prime; got {primed:?}",
        );
    }

    #[test]
    fn prime_ranks_by_savings_per_ref() {
        // Two equally-observed pairs but one has a longer value (bigger
        // per-reference savings). Capacity only fits one. Both names are static
        // (NameMatch), so cost model uses value-bytes savings.
        let observer = HeaderObserver::default();
        let big = (
            name(KnownHeaderName::ContentType),
            value(b"application/json; charset=utf-8"),
        );
        let small = (name(KnownHeaderName::ContentLength), value(b"12"));
        observe_once(&observer, &[big.clone(), small.clone()]);
        // Big entry size: 32 + 12 + 31 = 75.
        let primed = observer.prime(75, HeaderCompression::Qpack);
        assert_eq!(primed.len(), 1);
        assert_eq!(primed[0].name, big.0);
        assert_eq!(primed[0].value, Some(big.1));
    }

    #[test]
    fn high_cardinality_name_falls_back_to_name_only() {
        // Two distinct Static values for the same name within one connection
        // marks the name high-card; no pair entry survives in the accumulator,
        // but the name itself stays in the seen_names set.
        let mut accum = ConnectionAccumulator::default();
        accum.observe(&name(KnownHeaderName::Trailer), &value(b"value-a"));
        accum.observe(&name(KnownHeaderName::Trailer), &value(b"value-b"));
        let key = NameKey::Known(KnownHeaderName::Trailer);
        assert!(accum.high_card_names.contains(&key));
        assert!(!accum.seen_pairs.iter().any(|(k, _)| *k == key));
        assert!(accum.seen_names.contains(&key));
    }

    #[test]
    fn unknown_names_are_ignored() {
        // EntryName::Unknown (no static recovery) returns None from
        // name_key(), so the observer never sees them.
        let observer = HeaderObserver::default();
        let unknown: EntryName<'static> = EntryName::try_from(b"x-custom".to_vec()).unwrap();
        let mut accum = ConnectionAccumulator::default();
        accum.observe(&unknown, &value(b"hello"));
        assert!(accum.seen_pairs.is_empty());
        assert!(accum.seen_names.is_empty());
        observer.fold_connection(&accum);
        assert!(observer.prime(4096, HeaderCompression::Qpack).is_empty());
    }

    #[test]
    fn unknown_static_is_tracked() {
        let observer = HeaderObserver::default();
        let unknown_static = EntryName::UnknownStatic("x-trillium-flag");
        observe_once(&observer, &[(unknown_static.clone(), value(b"on"))]);
        let primed = observer.prime(4096, HeaderCompression::Qpack);
        assert!(
            primed
                .iter()
                .any(|c| c.name == unknown_static && c.value == Some(value(b"on"))),
            "UnknownStatic full-pair must prime; got {primed:?}",
        );
    }

    #[test]
    fn observe_skips_uncacheable_names() {
        let mut accum = ConnectionAccumulator::default();
        accum.observe(
            &name(KnownHeaderName::Authorization),
            &value(b"Bearer secret"),
        );
        // Name is recorded (for name-only priming consideration), but no pair.
        assert!(accum.seen_pairs.is_empty());
        let key = NameKey::Known(KnownHeaderName::Authorization);
        assert!(accum.seen_names.contains(&key));
    }

    #[test]
    fn observe_skips_non_static_values() {
        let mut accum = ConnectionAccumulator::default();
        accum.observe(
            &name(KnownHeaderName::Server),
            &FieldLineValue::Owned(b"trillium".to_vec()),
        );
        assert!(accum.seen_pairs.is_empty());
        let key = NameKey::Known(KnownHeaderName::Server);
        assert!(accum.seen_names.contains(&key));
    }

    #[test]
    fn fold_is_set_union() {
        // Two distinct pairs, each contributed by a separate connection. Both
        // names are static NameMatch only (no FullMatch), so both survive the
        // cost-model filter and end up in the primed set.
        let observer = HeaderObserver::default();
        let pair_a = (name(KnownHeaderName::Server), value(b"trillium"));
        let pair_b = (name(KnownHeaderName::UserAgent), value(b"test-agent/1.0"));
        observe_once(&observer, std::slice::from_ref(&pair_a));
        observe_once(&observer, std::slice::from_ref(&pair_b));
        let primed = observer.prime(4096, HeaderCompression::Qpack);
        assert!(
            primed
                .iter()
                .any(|c| c.name == pair_a.0 && c.value.as_ref() == Some(&pair_a.1))
        );
        assert!(
            primed
                .iter()
                .any(|c| c.name == pair_b.0 && c.value.as_ref() == Some(&pair_b.1))
        );
    }

    #[test]
    fn hpack_prime_emits_observed_pair() {
        // Same observation, costed under HPACK — both NameMatch arms produce
        // value-bytes savings, so the pair primes identically to QPACK.
        let observer = HeaderObserver::default();
        observe_once(
            &observer,
            &[(name(KnownHeaderName::Server), value(b"trillium"))],
        );
        let primed = observer.prime(4096, HeaderCompression::Hpack);
        assert_eq!(primed.len(), 1);
        assert_eq!(primed[0].name, name(KnownHeaderName::Server));
        assert_eq!(primed[0].value, Some(value(b"trillium")));
    }
}
