use super::PseudoHeaders;
use crate::{
    HeaderName, HeaderValue, Headers, KnownHeaderName, Method, Status,
    h3::{H3Connection, H3Error},
    headers::qpack::{FieldSection, decoder_dynamic_table::DecoderDynamicTable},
};
use std::borrow::Cow;
use test_harness::test;
use trillium_testing::harness;

#[track_caller]
fn roundtrip(
    pseudo_headers: PseudoHeaders<'_>,
    headers: &Headers,
) -> (PseudoHeaders<'static>, Headers) {
    let h3 = H3Connection::new(Default::default());
    let mut buf = Vec::new();
    let field_section = FieldSection::new(pseudo_headers, headers);
    let stream_id = 1;
    h3.encode_field_section(&field_section, &mut buf, stream_id)
        .unwrap();

    trillium_testing::block_on(async move {
        h3.decode_field_section(&buf, stream_id)
            .await
            .expect("decode failed")
            .into_parts()
    })
}

fn decode(buf: &[u8]) -> Result<FieldSection<'static>, H3Error> {
    let h3 = H3Connection::new(Default::default());
    let stream_id = 1;
    trillium_testing::block_on(h3.decode_field_section(buf, stream_id))
}

#[test]
fn roundtrip_all_static_matches() {
    let mut headers = Headers::new();
    headers.insert(KnownHeaderName::AcceptEncoding, "gzip, deflate, br");

    let (pseudos, decoded_headers) = roundtrip(
        PseudoHeaders {
            method: Some(Method::Get),
            path: Some(Cow::Borrowed("/")),
            scheme: Some(Cow::Borrowed("https")),
            ..Default::default()
        },
        &headers,
    );

    assert_eq!(pseudos.method, Some(Method::Get));
    assert_eq!(pseudos.path.as_deref(), Some("/"));
    assert_eq!(pseudos.scheme.as_deref(), Some("https"));
    assert_eq!(
        decoded_headers.get_str(KnownHeaderName::AcceptEncoding),
        Some("gzip, deflate, br"),
    );
}

#[test]
fn roundtrip_name_ref_with_custom_value() {
    let mut headers = Headers::new();
    headers.insert(KnownHeaderName::ContentType, "application/xml");
    headers.insert(KnownHeaderName::Server, "trillium");

    let (pseudos, decoded_headers) = roundtrip(
        PseudoHeaders {
            status: Some(Status::Ok),
            ..Default::default()
        },
        &headers,
    );

    assert_eq!(pseudos.status, Some(Status::Ok));
    assert_eq!(
        decoded_headers.get_str(KnownHeaderName::ContentType),
        Some("application/xml"),
    );
    assert_eq!(
        decoded_headers.get_str(KnownHeaderName::Server),
        Some("trillium"),
    );
}

#[test]
fn roundtrip_literal_name_and_value() {
    let mut headers = Headers::new();
    headers.insert("x-custom-header", "custom-value");

    let (pseudos, decoded_headers) = roundtrip(
        PseudoHeaders {
            method: Some(Method::Post),
            ..Default::default()
        },
        &headers,
    );

    assert_eq!(pseudos.method, Some(Method::Post));
    assert_eq!(
        decoded_headers.get_str("x-custom-header"),
        Some("custom-value"),
    );
}

#[test]
fn roundtrip_mixed() {
    let mut headers = Headers::new();
    headers.insert(KnownHeaderName::AcceptRanges, "bytes");
    headers.insert(KnownHeaderName::CacheControl, "private");
    headers.insert("x-request-id", "abc-123");

    let (pseudos, decoded_headers) = roundtrip(
        PseudoHeaders {
            method: Some(Method::Get),
            path: Some(Cow::Borrowed("/")),
            authority: Some(Cow::Borrowed("example.com")),
            ..Default::default()
        },
        &headers,
    );

    assert_eq!(pseudos.method, Some(Method::Get));
    assert_eq!(pseudos.path.as_deref(), Some("/"));
    assert_eq!(pseudos.authority.as_deref(), Some("example.com"));
    assert_eq!(
        decoded_headers.get_str(KnownHeaderName::AcceptRanges),
        Some("bytes"),
    );
    assert_eq!(
        decoded_headers.get_str(KnownHeaderName::CacheControl),
        Some("private"),
    );
    assert_eq!(decoded_headers.get_str("x-request-id"), Some("abc-123"));
}

// RFC 9204 §B.1 — Literal Field Line with Name Reference
// :path = /index.html (name ref to static table index 1, no huffman)
#[test]
fn rfc9204_b1_decode() {
    let bytes = &[
        0x00, 0x00, // prefix: required insert count=0, delta base=0
        0x51, 0x0b, 0x2f, 0x69, 0x6e, 0x64, 0x65, 0x78, 0x2e, 0x68, 0x74, 0x6d, 0x6c,
    ];

    let (pseudos, headers) = decode(bytes).expect("decode failed").into_parts();
    assert_eq!(pseudos.path.as_deref(), Some("/index.html"));
    assert!(headers.is_empty());

    // Verify round-trip through our encoder
    let (pseudos2, _) = roundtrip(
        PseudoHeaders {
            path: Some(Cow::Borrowed("/index.html")),
            ..Default::default()
        },
        &Headers::new(),
    );
    assert_eq!(pseudos2.path.as_deref(), Some("/index.html"));
}

#[test]
fn roundtrip_all_static_table_methods() {
    let headers = Headers::new();
    for method in [
        Method::Connect,
        Method::Delete,
        Method::Get,
        Method::Head,
        Method::Options,
        Method::Post,
        Method::Put,
    ] {
        let (pseudos, _) = roundtrip(
            PseudoHeaders {
                method: Some(method),
                ..Default::default()
            },
            &headers,
        );
        assert_eq!(pseudos.method, Some(method), "method {method}");
    }
}

#[test]
fn roundtrip_non_standard_method() {
    let (pseudos, _) = roundtrip(
        PseudoHeaders {
            method: Some(Method::Patch),
            ..Default::default()
        },
        &Headers::new(),
    );
    assert_eq!(pseudos.method, Some(Method::Patch));
}

#[test]
fn roundtrip_all_static_table_statuses() {
    let headers = Headers::new();
    for status in [
        Status::Continue,
        Status::EarlyHints,
        Status::Ok,
        Status::NoContent,
        Status::PartialContent,
        Status::Found,
        Status::NotModified,
        Status::BadRequest,
        Status::Forbidden,
        Status::NotFound,
        Status::MisdirectedRequest,
        Status::TooEarly,
        Status::InternalServerError,
        Status::ServiceUnavailable,
    ] {
        let (pseudos, _) = roundtrip(
            PseudoHeaders {
                status: Some(status),
                ..Default::default()
            },
            &headers,
        );
        assert_eq!(pseudos.status, Some(status), "status {status}");
    }
}

#[test]
fn roundtrip_non_standard_status() {
    let (pseudos, _) = roundtrip(
        PseudoHeaders {
            status: Some(Status::ImATeapot),
            ..Default::default()
        },
        &Headers::new(),
    );
    assert_eq!(pseudos.status, Some(Status::ImATeapot));
}

#[test]
fn roundtrip_empty_header_value() {
    let mut headers = Headers::new();
    headers.insert(KnownHeaderName::Server, "");
    let (_, decoded) = roundtrip(PseudoHeaders::default(), &headers);
    assert_eq!(decoded.get_str(KnownHeaderName::Server), Some(""));
}

#[test]
fn roundtrip_long_header_value() {
    let long_value = "x".repeat(300);
    let mut headers = Headers::new();
    headers.insert("x-long", HeaderValue::from(long_value.clone()));
    let (_, decoded) = roundtrip(PseudoHeaders::default(), &headers);
    assert_eq!(decoded.get_str("x-long"), Some(long_value.as_str()));
}

// --- Decoder error path tests ---

#[test]
fn decode_empty_input() {
    assert!(decode(&[]).is_err());
}

#[test]
fn decode_truncated_prefix() {
    assert!(decode(&[0x00]).is_err());
}

#[test]
fn decode_nonzero_required_insert_count() {
    assert!(decode(&[0x01, 0x00]).is_err());
}

#[test]
fn decode_dynamic_table_indexed() {
    assert!(decode(&[0x00, 0x00, 0x10]).is_err());
}

#[test]
fn decode_dynamic_table_name_ref() {
    assert!(decode(&[0x00, 0x00, 0x00]).is_err());
}

#[test]
fn decode_static_index_out_of_range() {
    let buf = vec![0x00, 0x00, 0xFF, 0x24];
    assert!(decode(&buf).is_err());
}

#[test]
fn decode_truncated_string_value() {
    let buf = vec![
        0x00, 0x00, // prefix
        0x51, // literal with name ref, static, index 1
        0x05, // string length 5, no huffman
        0x2f, 0x61, // only 2 bytes of the promised 5
    ];
    assert!(decode(&buf).is_err());
}

// --- Malformed-message detection (RFC 9114 §4.1.1) ---

#[test]
fn decode_rejects_duplicate_pseudo_header() {
    // RIC=0, base=0, then :method GET (static idx 17, 0xD1), then :method POST (static idx 20,
    // 0xD4). Two `:method` pseudo-headers in one section MUST be treated as malformed (RFC 9114
    // §4.1.1), mirroring HPACK's `DuplicatePseudoHeader` detection.
    let buf = vec![0x00, 0x00, 0xD1, 0xD4];
    let err = decode(&buf).expect_err("duplicate :method must fail");
    let h3_err = match err {
        H3Error::Protocol(code) => code,
        other => panic!("expected Protocol error, got {other:?}"),
    };
    assert_eq!(h3_err, crate::h3::H3ErrorCode::MessageError);
}

#[test]
fn decode_rejects_pseudo_after_regular() {
    // RIC=0, base=0, then accept-encoding "gzip, deflate, br" (static idx 31, 0xDF), then
    // :method GET (static idx 17, 0xD1). RFC 9114 §4.3.1: pseudo-headers MUST appear before
    // any regular header field; a section that violates this MUST be treated as malformed.
    let buf = vec![0x00, 0x00, 0xDF, 0xD1];
    let err = decode(&buf).expect_err("pseudo-after-regular must fail");
    let h3_err = match err {
        H3Error::Protocol(code) => code,
        other => panic!("expected Protocol error, got {other:?}"),
    };
    assert_eq!(h3_err, crate::h3::H3ErrorCode::MessageError);
}

#[test]
fn decode_accepts_distinct_pseudos_in_order() {
    // Sanity check that the strict-detection path doesn't misfire on a well-formed section.
    // RIC=0, base=0, :method GET (0xD1), :path "/" (static idx 1, 0xC1).
    let buf = vec![0x00, 0x00, 0xD1, 0xC1];
    let (pseudos, _) = decode(&buf).expect("well-formed must decode").into_parts();
    assert_eq!(pseudos.method, Some(Method::Get));
    assert_eq!(pseudos.path.as_deref(), Some("/"));
}

// --- Dynamic table: blocked-streams enforcement ---

#[test]
fn blocked_streams_not_triggered_for_zero_ric() {
    let table = DecoderDynamicTable::new(4096, 0);
    // RIC=0 means static-only; no blocking regardless of limit
    assert!(table.try_reserve_blocked_stream(0).unwrap().is_none());
}

#[test]
fn blocked_streams_enforced_at_limit() {
    let table = DecoderDynamicTable::new(4096, 0);
    // max_blocked=0, insert_count=0 < RIC=1 → at limit immediately
    assert!(table.try_reserve_blocked_stream(1).is_err());
}

#[test]
fn blocked_stream_guard_releases_slot_on_drop() {
    let table = DecoderDynamicTable::new(4096, 1);
    let guard = table.try_reserve_blocked_stream(1).unwrap();
    assert!(guard.is_some());
    // Limit reached: second reservation fails
    assert!(table.try_reserve_blocked_stream(1).is_err());
    drop(guard);
    // Slot released: succeeds again
    assert!(table.try_reserve_blocked_stream(1).unwrap().is_some());
}

#[test]
fn blocked_streams_not_triggered_when_ric_already_met() {
    let table = DecoderDynamicTable::new(4096, 0);
    table.set_capacity(200).unwrap();
    // "server"(6) + "v"(1) + 32 = 39 bytes — fits easily
    table
        .insert(
            HeaderName::from(KnownHeaderName::Server),
            Cow::Owned(b"v".to_vec()),
        )
        .unwrap();
    // insert_count=1 >= RIC=1, so no blocking needed even with max_blocked=0
    assert!(table.try_reserve_blocked_stream(1).unwrap().is_none());
}

#[test(harness)]
async fn decode_rejects_blocked_stream_when_at_limit() {
    // Encoded field section with RIC=1 referencing a dynamic table entry not yet inserted.
    // Byte layout (max_capacity=4096, so max_entries=128, full_range=256):
    //   0x02 — encoded_ric=2, decodes to required_insert_count=1
    //   0x00 — S=0, delta_base=0 → base=1
    //   0x80 — indexed field line (bit7=1), dynamic (bit6=0), relative_index=0
    //          → absolute_index = base-1-0 = 0, would block waiting for insert_count>=1
    let encoded = [0x02u8, 0x00, 0x80];
    let table = DecoderDynamicTable::new(4096, 0); // max_blocked_streams=0
    let result = table.decode(&encoded, 0).await;
    assert!(result.is_err());
}

// --- Dynamic table: eviction ---

#[test(harness)]
async fn dynamic_table_evicts_oldest_entry() {
    // Entry size: "server"(6) + "val"(3) + 32 = 41 bytes; capacity=80 holds exactly one.
    let table = DecoderDynamicTable::new(200, usize::MAX);
    table.set_capacity(80).unwrap();
    let name = || HeaderName::from(KnownHeaderName::Server);
    table.insert(name(), Cow::Owned(b"first".to_vec())).unwrap(); // abs=0
    table
        .insert(name(), Cow::Owned(b"second".to_vec()))
        .unwrap(); // abs=1, evicts abs=0

    let (_, value) = table.get(1, 2).await.unwrap();
    assert_eq!(value.as_ref(), b"second");
    assert!(table.get(0, 2).await.is_err()); // evicted
}

#[test(harness)]
async fn dynamic_table_evicts_multiple_entries_for_large_insert() {
    // Two small entries (41 bytes each = 82 bytes), then a larger entry (72 bytes) that
    // requires evicting both to fit within capacity=100.
    //   "x-big-name"(10) + "x"*30(30) + 32 = 72 bytes
    let table = DecoderDynamicTable::new(4096, usize::MAX);
    table.set_capacity(100).unwrap();
    let small_name = || HeaderName::from(KnownHeaderName::Server);
    let big_name = HeaderName::from("x-big-name".to_owned());
    table
        .insert(small_name(), Cow::Owned(b"val".to_vec()))
        .unwrap(); // abs=0, 41 bytes
    table
        .insert(small_name(), Cow::Owned(b"val".to_vec()))
        .unwrap(); // abs=1, 41 bytes — total 82

    let big_value = Cow::Owned(b"x".repeat(30));
    table.insert(big_name, big_value).unwrap(); // abs=2, 72 bytes — evicts abs=0 then abs=1

    assert!(table.get(0, 3).await.is_err()); // evicted
    assert!(table.get(1, 3).await.is_err()); // evicted
    let (name, _) = table.get(2, 3).await.unwrap();
    assert_eq!(name.as_ref(), "x-big-name");
}

// --- RFC 9204 §4.5.4 N (Never-Indexed) bit round-trip ---

#[test]
fn never_indexed_round_trips_through_headers() {
    // App-side: build a value with the N bit set, encode through the QPACK encoder,
    // decode it back, and verify the bit survives. Mirrors the proxy round-trip path
    // (decode-then-re-encode) at the encoder/decoder seam.
    let mut headers = Headers::new();
    let mut secret = HeaderValue::from("Bearer abc123");
    secret.set_never_indexed(true);
    headers.insert(KnownHeaderName::Authorization, secret);
    headers.insert(KnownHeaderName::ContentType, "application/json");

    let (_, decoded_headers) = roundtrip(
        PseudoHeaders {
            method: Some(Method::Get),
            path: Some(Cow::Borrowed("/")),
            scheme: Some(Cow::Borrowed("https")),
            ..Default::default()
        },
        &headers,
    );

    let auth = decoded_headers
        .get_values(KnownHeaderName::Authorization)
        .expect("authorization present");
    assert_eq!(
        auth.one().and_then(HeaderValue::as_str),
        Some("Bearer abc123")
    );
    assert!(
        auth.iter().all(HeaderValue::is_never_indexed),
        "N bit must survive QPACK round-trip on the secret value",
    );

    let ct = decoded_headers
        .get_values(KnownHeaderName::ContentType)
        .expect("content-type present");
    assert!(
        ct.iter().all(|v| !v.is_never_indexed()),
        "non-secret value must not pick up the N bit",
    );
}

#[test]
fn never_indexed_emits_literal_for_static_full_match() {
    // RFC 9204 §4.5.4: when N=1 the encoded representation MUST be literal. Even when the
    // (name, value) pair has a static-table full match, the encoder must skip the indexed
    // shortcut and emit a literal-with-name-ref.
    let mut headers = Headers::new();
    // ":path /" is static table index 1 (full match) — but we route through a regular
    // header here. Use a value that is a known static full-match in the qpack static
    // table: "accept-encoding: gzip, deflate, br" (idx 31).
    let mut value = HeaderValue::from("gzip, deflate, br");
    value.set_never_indexed(true);
    headers.insert(KnownHeaderName::AcceptEncoding, value);

    let h3 = H3Connection::new(Default::default());
    let mut buf = Vec::new();
    let field_section = FieldSection::new(PseudoHeaders::default(), &headers);
    h3.encode_field_section(&field_section, &mut buf, 1)
        .unwrap();

    // Wire bytes after the 2-byte section prefix should NOT be a §4.5.2 indexed-static
    // representation (top bit + T-bit pattern: 1Txxxxxx with T=1). Instead they must be
    // a §4.5.4 literal-with-static-name-ref with the N bit set (01N1xxxx pattern).
    let body = &buf[2..];
    let first = body[0];
    assert_eq!(
        first & 0b1100_0000,
        0b0100_0000,
        "expected §4.5.4 literal-with-name-ref (01xxxxxx), got first byte {first:#04x}",
    );
    assert_eq!(
        first & 0b0010_0000,
        0b0010_0000,
        "N bit must be set on §4.5.4 emission, got first byte {first:#04x}",
    );

    // And the round-trip preserves the bit.
    let decoded = decode(&buf).expect("round-trip decode").into_parts().1;
    let v = decoded
        .get_values(KnownHeaderName::AcceptEncoding)
        .expect("accept-encoding present");
    assert!(v.iter().all(HeaderValue::is_never_indexed));
}
