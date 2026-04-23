//! Smoke tests for the HPACK dynamic-table encoder. Roundtrip-style: encode with
//! [`HpackEncoder::encode`], decode with [`HpackDecoder`], assert the field section
//! is recovered.
//!
//! [`HpackDecoder`]: crate::headers::hpack::HpackDecoder

use super::HpackEncoder;
use crate::{
    KnownHeaderName,
    headers::{
        Headers,
        entry_name::EntryName,
        field_section::{FieldLineValue, FieldSection, PseudoHeaders},
        header_observer::{ConnectionAccumulator, HeaderObserver},
        hpack::HpackDecoder,
    },
};
use std::sync::Arc;

fn observer() -> Arc<HeaderObserver> {
    Arc::default()
}

/// Encode one block, returning the wire bytes.
fn encode(enc: &mut HpackEncoder, section: &FieldSection<'_>) -> Vec<u8> {
    let mut buf = Vec::new();
    enc.encode(section, &mut buf);
    buf
}

fn entries_len(enc: &HpackEncoder) -> usize {
    enc.state.entries.len()
}

fn insert_count(enc: &HpackEncoder) -> u64 {
    enc.state.insert_count
}

fn current_size(enc: &HpackEncoder) -> usize {
    enc.state.current_size
}

#[test]
fn full_static_match_roundtrips() {
    // Encoded as §6.1 IndexedStatic with no dynamic-table interaction.
    let mut enc = HpackEncoder::new(observer(), 4096, 16);
    let headers =
        Headers::new().with_inserted_header(KnownHeaderName::AcceptEncoding, "gzip, deflate");
    let section = FieldSection::new(PseudoHeaders::default(), &headers);

    let buf = encode(&mut enc, &section);

    let mut dec = HpackDecoder::new(4096);
    let decoded = dec.decode(&buf).unwrap();
    assert_eq!(
        decoded.headers().get_str(KnownHeaderName::AcceptEncoding),
        Some("gzip, deflate"),
    );
}

#[test]
fn name_match_only_uses_literal_without_indexing_on_first_sight() {
    // First sighting of (server, trillium): not yet in recent_pairs, observer cold,
    // so we emit §6.2.2. Decoder roundtrip succeeds and the dynamic table stays empty.
    let mut enc = HpackEncoder::new(observer(), 4096, 16);
    let headers = Headers::new().with_inserted_header(KnownHeaderName::Server, "trillium");
    let section = FieldSection::new(PseudoHeaders::default(), &headers);

    let buf = encode(&mut enc, &section);

    let mut dec = HpackDecoder::new(4096);
    let decoded = dec.decode(&buf).unwrap();
    assert_eq!(
        decoded.headers().get_str(KnownHeaderName::Server),
        Some("trillium"),
    );
    // Encoder didn't insert (first sighting; recent_pairs miss).
    assert_eq!(entries_len(&enc), 0);
}

#[test]
fn second_sighting_promotes_to_indexed_dynamic() {
    // Block 1: emit (server, trillium) as §6.2.2 — first sighting.
    // Block 2: emit (server, trillium) again — recent_pairs HIT → §6.2.1 inserts;
    // decoder now has it in dynamic table.
    // Block 3: emit (server, trillium) again — full match → §6.1 IndexedDynamic.
    let mut enc = HpackEncoder::new(observer(), 4096, 16);
    let headers = Headers::new().with_inserted_header(KnownHeaderName::Server, "trillium");

    let mut dec = HpackDecoder::new(4096);

    for _ in 0..3 {
        let section = FieldSection::new(PseudoHeaders::default(), &headers);
        let buf = encode(&mut enc, &section);
        let decoded = dec.decode(&buf).unwrap();
        assert_eq!(
            decoded.headers().get_str(KnownHeaderName::Server),
            Some("trillium"),
        );
    }
    assert_eq!(entries_len(&enc), 1);
    assert_eq!(insert_count(&enc), 1);
}

#[test]
fn observer_hot_promotes_on_first_connection_sighting() {
    // Pre-fold a (server, trillium) observation into the shared observer (simulating
    // a prior connection on this listener). On a brand-new encoder, the very first
    // sighting should hit observer.is_hot → §6.2.1 insert on first sight.
    let observer = observer();
    let mut accum = ConnectionAccumulator::default();
    accum.observe(
        &EntryName::Known(KnownHeaderName::Server),
        &FieldLineValue::Static(b"trillium"),
    );
    observer.fold_connection(&accum);

    let mut enc = HpackEncoder::new(observer, 4096, 16);
    let headers = Headers::new().with_inserted_header(KnownHeaderName::Server, "trillium");
    let section = FieldSection::new(PseudoHeaders::default(), &headers);

    let buf = encode(&mut enc, &section);

    // Observer-hot first sighting → insert happened.
    assert_eq!(entries_len(&enc), 1);

    let mut dec = HpackDecoder::new(4096);
    let decoded = dec.decode(&buf).unwrap();
    assert_eq!(
        decoded.headers().get_str(KnownHeaderName::Server),
        Some("trillium"),
    );
}

#[test]
fn oversized_entry_clears_table_per_4_4() {
    // §4.4: an entry whose own §4.1 size exceeds max_size clears the dynamic table
    // and is not stored. Set max_size small, seed an entry, then index a too-large
    // pair — that should clear the table.
    let observer = observer();
    {
        // Seed both pairs in the observer so both pass the should_index gate.
        let mut accum = ConnectionAccumulator::default();
        accum.observe(
            &EntryName::Known(KnownHeaderName::Server),
            &FieldLineValue::Static(b"trillium"),
        );
        accum.observe(
            &EntryName::UnknownStatic("x-large"),
            &FieldLineValue::Static(b"BIG"),
        );
        observer.fold_connection(&accum);
    }

    let mut enc = HpackEncoder::new(observer, 64, 16);

    let h1 = Headers::new().with_inserted_header(KnownHeaderName::Server, "trillium");
    let s1 = FieldSection::new(PseudoHeaders::default(), &h1);
    let buf1 = encode(&mut enc, &s1);
    assert_eq!(entries_len(&enc), 1);

    // Pair whose size alone exceeds max_size: 32 + 7 + 200 = 239 > 64.
    let big_value: String = "x".repeat(200);
    let h2 = Headers::new().with_inserted_header(
        crate::headers::HeaderName::from("x-large"),
        big_value.clone(),
    );
    let s2 = FieldSection::new(PseudoHeaders::default(), &h2);
    let buf2 = encode(&mut enc, &s2);

    // §4.4 clear fires inside `state.insert` — table empty afterwards.
    assert_eq!(entries_len(&enc), 0);
    assert_eq!(current_size(&enc), 0);

    // Decoder roundtrip on both blocks.
    let mut dec = HpackDecoder::new(64);
    assert_eq!(
        dec.decode(&buf1)
            .unwrap()
            .headers()
            .get_str(KnownHeaderName::Server),
        Some("trillium"),
    );
    let decoded2 = dec.decode(&buf2).unwrap();
    assert_eq!(
        decoded2
            .headers()
            .get_str(crate::headers::HeaderName::from("x-large")),
        Some(big_value.as_str()),
    );
}

#[test]
fn intra_block_eviction_resolved_inline() {
    // Single block with two §6.2.1 inserts where the second insert evicts the entry
    // referenced by the first — proves the encoder sees live state for each line.
    // With max_size = 50 and (server, trillium) = 46 bytes, the table fits one
    // entry. Inserting the second pair evicts the first.
    let observer = observer();
    {
        let mut accum = ConnectionAccumulator::default();
        accum.observe(
            &EntryName::Known(KnownHeaderName::Server),
            &FieldLineValue::Static(b"trillium"),
        );
        accum.observe(
            &EntryName::Known(KnownHeaderName::Etag),
            &FieldLineValue::Static(b"abc123"),
        );
        observer.fold_connection(&accum);
    }

    let mut enc = HpackEncoder::new(observer, 50, 16);

    let mut headers = Headers::new();
    headers.append(KnownHeaderName::Server, "trillium");
    headers.append(KnownHeaderName::Etag, "abc123");
    let section = FieldSection::new(PseudoHeaders::default(), &headers);

    let buf = encode(&mut enc, &section);

    // Decoder roundtrip: both lines must come back.
    let mut dec = HpackDecoder::new(50);
    let decoded = dec.decode(&buf).unwrap();
    assert_eq!(
        decoded.headers().get_str(KnownHeaderName::Server),
        Some("trillium"),
    );
    assert_eq!(
        decoded.headers().get_str(KnownHeaderName::Etag),
        Some("abc123"),
    );
    // Only the most recent insert survives the eviction churn.
    assert_eq!(entries_len(&enc), 1);
}
