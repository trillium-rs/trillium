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
            |c| matches!(c.name, EntryName::Pseudo(PseudoHeaderName::Status)) && c.value.is_some()
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
