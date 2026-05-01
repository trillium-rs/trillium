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

/// Construct an encoder and immediately raise its operational size to `local_pref` by
/// simulating a peer SETTINGS frame advertising `u32::MAX`. Tests that don't care about
/// the §6.3 startup dance use this; tests that exercise the dance directly use
/// [`HpackEncoder::new`] + [`HpackEncoder::set_protocol_max_size`] explicitly.
fn new_encoder(
    observer: Arc<HeaderObserver>,
    local_pref: usize,
    recent_pairs: usize,
) -> HpackEncoder {
    let mut enc = HpackEncoder::new(observer, local_pref, recent_pairs);
    enc.set_protocol_max_size(usize::MAX);
    enc
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
    let mut enc = new_encoder(observer(), 4096, 16);
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
    let mut enc = new_encoder(observer(), 4096, 16);
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
    let mut enc = new_encoder(observer(), 4096, 16);
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

    let mut enc = new_encoder(observer, 4096, 16);
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

    let mut enc = new_encoder(observer, 64, 16);

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

    let mut enc = new_encoder(observer, 50, 16);

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

#[test]
fn pre_settings_encoder_never_inserts() {
    // Without `set_protocol_max_size` the operational size stays at 0; even an
    // observer-hot pair fails §4.4 (entry size > 0) and the table stays empty.
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

    assert_eq!(entries_len(&enc), 0);

    // Decoder still roundtrips — emission falls back to §6.2.2 literal-without-indexing.
    let mut dec = HpackDecoder::new(4096);
    let decoded = dec.decode(&buf).unwrap();
    assert_eq!(
        decoded.headers().get_str(KnownHeaderName::Server),
        Some("trillium"),
    );
}

#[test]
fn protocol_max_settings_emits_size_update() {
    // After `set_protocol_max_size(4096)`, the next encode prepends a §6.3 instruction
    // whose new-max = min(local_pref, peer_advertised) = 4096. Decoder's protocol max is
    // 4096 too, so the §6.3 is accepted; without the §6.3, the decoder would treat any
    // dynamic-table inserts as §4.2 violations (size update MUST occur at start of next
    // section after a change).
    let mut enc = HpackEncoder::new(observer(), 4096, 16);
    enc.set_protocol_max_size(4096);

    let headers = Headers::new().with_inserted_header(KnownHeaderName::AcceptEncoding, "gzip");
    let section = FieldSection::new(PseudoHeaders::default(), &headers);
    let buf = encode(&mut enc, &section);

    // First byte: 001xxxxx pattern (0x20-0x3F). 4096 in 5-bit prefix is the multi-byte
    // form (4096 > 30), so first byte is 0011_1111 = 0x3F.
    assert_eq!(buf[0], 0x3F);

    // Roundtrip succeeds — the decoder accepts the §6.3.
    let mut dec = HpackDecoder::new(4096);
    let decoded = dec.decode(&buf).unwrap();
    assert_eq!(
        decoded.headers().get_str(KnownHeaderName::AcceptEncoding),
        Some("gzip"),
    );
}

#[test]
fn protocol_max_caps_at_local_preferred() {
    // Local preferred = 1024; peer advertises 8192. Operational ends up at 1024.
    // Verify: encoder respects 1024, decoder constructed at 1024 accepts the §6.3.
    let observer = observer();
    let mut accum = ConnectionAccumulator::default();
    accum.observe(
        &EntryName::Known(KnownHeaderName::Server),
        &FieldLineValue::Static(b"trillium"),
    );
    observer.fold_connection(&accum);

    let mut enc = HpackEncoder::new(observer, 1024, 16);
    enc.set_protocol_max_size(8192);

    let headers = Headers::new().with_inserted_header(KnownHeaderName::Server, "trillium");
    let section = FieldSection::new(PseudoHeaders::default(), &headers);
    let buf = encode(&mut enc, &section);

    // Decoder advertised 1024; the §6.3 prepended by the encoder has value 1024 (the
    // capped operational size, not the peer's 8192). Without the cap, the decoder would
    // reject §6.3 with new_max > protocol_max_table_size.
    let mut dec = HpackDecoder::new(1024);
    let decoded = dec.decode(&buf).unwrap();
    assert_eq!(
        decoded.headers().get_str(KnownHeaderName::Server),
        Some("trillium"),
    );
    // Insert happened at 1024, not 8192.
    assert_eq!(entries_len(&enc), 1);
}

#[test]
fn protocol_max_shrink_evicts_and_emits_size_update() {
    // Bring encoder to 1024, insert two entries, shrink to a size that fits only one.
    // The §6.3 size update propagates and the decoder evicts in lockstep.
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
    let mut enc = HpackEncoder::new(observer, 1024, 16);
    enc.set_protocol_max_size(1024);

    // Block 1: insert (server, trillium) and (etag, abc123).
    let mut h1 = Headers::new();
    h1.append(KnownHeaderName::Server, "trillium");
    h1.append(KnownHeaderName::Etag, "abc123");
    let s1 = FieldSection::new(PseudoHeaders::default(), &h1);
    let buf1 = encode(&mut enc, &s1);
    assert_eq!(entries_len(&enc), 2);

    // (server, trillium): 32 + 6 + 8 = 46. (etag, abc123): 32 + 4 + 6 = 42. Total 88.
    // Shrink to 50 — must evict the older (server, trillium) entry to fit the newer one.
    enc.set_protocol_max_size(50);
    assert_eq!(entries_len(&enc), 1);

    // Block 2: any encode triggers the queued §6.3 with new max 50.
    let h2 = Headers::new().with_inserted_header(KnownHeaderName::AcceptEncoding, "gzip");
    let s2 = FieldSection::new(PseudoHeaders::default(), &h2);
    let buf2 = encode(&mut enc, &s2);
    // §6.3 prefix `001xxxxx` in the top 3 bits.
    assert_eq!(buf2[0] & 0b1110_0000, 0b0010_0000);

    // Decoder must follow along.
    let mut dec = HpackDecoder::new(1024);
    let _ = dec.decode(&buf1).unwrap();
    let _ = dec.decode(&buf2).unwrap();
}

#[test]
fn protocol_max_zero_clears_table() {
    // Peer advertises 0 — encoder must evict everything and emit §6.3 0.
    let observer = observer();
    let mut accum = ConnectionAccumulator::default();
    accum.observe(
        &EntryName::Known(KnownHeaderName::Server),
        &FieldLineValue::Static(b"trillium"),
    );
    observer.fold_connection(&accum);

    let mut enc = HpackEncoder::new(observer, 4096, 16);
    enc.set_protocol_max_size(4096);

    // Seed an entry.
    let h1 = Headers::new().with_inserted_header(KnownHeaderName::Server, "trillium");
    let s1 = FieldSection::new(PseudoHeaders::default(), &h1);
    let buf1 = encode(&mut enc, &s1);
    assert_eq!(entries_len(&enc), 1);

    // Peer drops to 0. Encoder evicts and queues §6.3 0.
    enc.set_protocol_max_size(0);
    assert_eq!(entries_len(&enc), 0);

    let buf2 = encode(&mut enc, &s1);
    // First byte: §6.3 prefix 001 + value 0 in low 5 bits = 0010_0000 = 0x20.
    assert_eq!(buf2[0], 0x20);

    // Decoder roundtrip on both blocks.
    let mut dec = HpackDecoder::new(4096);
    let _ = dec.decode(&buf1).unwrap();
    let _ = dec.decode(&buf2).unwrap();
}

#[test]
fn protocol_max_idempotent() {
    // Repeated `set_protocol_max_size` with the same value is a no-op (no §6.3 re-emit).
    let mut enc = HpackEncoder::new(observer(), 4096, 16);
    enc.set_protocol_max_size(4096);
    let headers = Headers::new().with_inserted_header(KnownHeaderName::AcceptEncoding, "gzip");
    let section = FieldSection::new(PseudoHeaders::default(), &headers);
    let _ = encode(&mut enc, &section); // drains the initial §6.3

    enc.set_protocol_max_size(4096); // idempotent — no new §6.3 queued
    let buf = encode(&mut enc, &section);
    // First byte must NOT start with §6.3 prefix `001xxxxx` — i.e. no size update emitted.
    assert_ne!(buf[0] & 0b1110_0000, 0b0010_0000);
}

// --- RFC 7541 §6.2.3 N (Never-Indexed) bit round-trip ---

#[test]
fn never_indexed_round_trips_through_headers() {
    // Encode a HeaderValue marked never-indexed, decode, verify the bit survives. Mirrors
    // the proxy round-trip path at the encoder/decoder seam.
    use crate::HeaderValue;

    let mut enc = new_encoder(observer(), 4096, 16);
    let mut secret = HeaderValue::from("Bearer abc123");
    secret.set_never_indexed(true);
    let mut headers = Headers::new();
    headers.insert(KnownHeaderName::Authorization, secret);
    headers.insert(KnownHeaderName::ContentType, "application/json");
    let section = FieldSection::new(PseudoHeaders::default(), &headers);

    let buf = encode(&mut enc, &section);

    let mut dec = HpackDecoder::new(4096);
    let decoded = dec.decode(&buf).unwrap();
    let auth = decoded
        .headers()
        .get_values(KnownHeaderName::Authorization)
        .expect("authorization present");
    assert_eq!(
        auth.one().and_then(HeaderValue::as_str),
        Some("Bearer abc123")
    );
    assert!(
        auth.iter().all(HeaderValue::is_never_indexed),
        "N bit must survive HPACK round-trip on the secret value",
    );
    let ct = decoded
        .headers()
        .get_values(KnownHeaderName::ContentType)
        .expect("content-type present");
    assert!(
        ct.iter().all(|v| !v.is_never_indexed()),
        "non-secret value must not pick up the N bit",
    );

    // Encoder must not have inserted a never-indexed value into its dynamic table.
    assert_eq!(entries_len(&enc), 0);
}

#[test]
fn never_indexed_emits_section_6_2_3_for_static_full_match() {
    // RFC 7541 §6.2.3: when N=1, MUST emit a literal representation. The encoder must
    // skip the §6.1 IndexedStatic shortcut even when the (name, value) is a full
    // static-table match, and must use the §6.2.3 wire encoding (`0001xxxx` prefix).
    use crate::HeaderValue;

    let mut enc = new_encoder(observer(), 4096, 16);
    // Drain the initial §6.3 size update emitted on the first encode by encoding an
    // empty section first.
    let _ = encode(
        &mut enc,
        &FieldSection::new(PseudoHeaders::default(), &Headers::new()),
    );

    let mut value = HeaderValue::from("gzip, deflate");
    value.set_never_indexed(true);
    let headers = Headers::new().with_inserted_header(KnownHeaderName::AcceptEncoding, value);
    let section = FieldSection::new(PseudoHeaders::default(), &headers);

    let buf = encode(&mut enc, &section);
    // §6.2.3 prefix is `0001xxxx` (top 4 bits).
    assert_eq!(
        buf[0] & 0b1111_0000,
        0b0001_0000,
        "expected §6.2.3 LiteralNeverIndexed (0001xxxx), got first byte {:#04x}",
        buf[0],
    );

    // Round-trip preserves the bit.
    let mut dec = HpackDecoder::new(4096);
    let decoded = dec.decode(&buf).unwrap();
    let v = decoded
        .headers()
        .get_values(KnownHeaderName::AcceptEncoding)
        .expect("accept-encoding present");
    assert!(v.iter().all(HeaderValue::is_never_indexed));
    assert_eq!(entries_len(&enc), 0);
}
