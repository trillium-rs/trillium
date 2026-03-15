use super::{decode::decode_bitwise, *};

// C.4.1
#[test]
fn encode_www_example_com() {
    assert_eq!(
        encode(b"www.example.com"),
        vec![
            0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90, 0xf4, 0xff
        ]
    );
}

// C.4.2
#[test]
fn encode_no_cache() {
    assert_eq!(
        encode(b"no-cache"),
        vec![0xa8, 0xeb, 0x10, 0x64, 0x9c, 0xbf]
    );
}

// C.4.3
#[test]
fn encode_custom_key() {
    assert_eq!(
        encode(b"custom-key"),
        vec![0x25, 0xa8, 0x49, 0xe9, 0x5b, 0xa9, 0x7d, 0x7f]
    );
}

// C.4.3
#[test]
fn encode_custom_value() {
    assert_eq!(
        encode(b"custom-value"),
        vec![0x25, 0xa8, 0x49, 0xe9, 0x5b, 0xb8, 0xe8, 0xb4, 0xbf]
    );
}

// C.6.1
#[test]
fn encode_302() {
    assert_eq!(encode(b"302"), vec![0x64, 0x02]);
}

// C.6.1
#[test]
fn encode_private() {
    assert_eq!(encode(b"private"), vec![0xae, 0xc3, 0x77, 0x1a, 0x4b]);
}

// C.6.1
#[test]
fn encode_mon_21_oct_2013_20_13_21_gmt() {
    assert_eq!(
        encode(b"Mon, 21 Oct 2013 20:13:21 GMT"),
        vec![
            0xd0, 0x7a, 0xbe, 0x94, 0x10, 0x54, 0xd4, 0x44, 0xa8, 0x20, 0x05, 0x95, 0x04, 0x0b,
            0x81, 0x66, 0xe0, 0x82, 0xa6, 0x2d, 0x1b, 0xff
        ]
    );
}

// C.6.1
#[test]
fn encode_https_www_example_com() {
    assert_eq!(
        encode(b"https://www.example.com"),
        vec![
            0x9d, 0x29, 0xad, 0x17, 0x18, 0x63, 0xc7, 0x8f, 0x0b, 0x97, 0xc8, 0xe9, 0xae, 0x82,
            0xae, 0x43, 0xd3
        ]
    );
}

// C.6.2
#[test]
fn encode_307() {
    assert_eq!(encode(b"307"), vec![0x64, 0x0e, 0xff]);
}

// C.6.3
#[test]
fn encode_gzip() {
    assert_eq!(encode(b"gzip"), vec![0x9b, 0xd9, 0xab]);
}

// C.6.3
#[test]
fn encode_set_cookie_value() {
    assert_eq!(
        encode(b"foo=ASDJKHQKBZXOQWEOPIUAXQWEOIU; max-age=3600; version=1"),
        vec![
            0x94, 0xe7, 0x82, 0x1d, 0xd7, 0xf2, 0xe6, 0xc7, 0xb3, 0x35, 0xdf, 0xdf, 0xcd, 0x5b,
            0x39, 0x60, 0xd5, 0xaf, 0x27, 0x08, 0x7f, 0x36, 0x72, 0xc1, 0xab, 0x27, 0x0f, 0xb5,
            0x29, 0x1f, 0x95, 0x87, 0x31, 0x60, 0x65, 0xc0, 0x03, 0xed, 0x4e, 0xe5, 0xb1, 0x06,
            0x3d, 0x50, 0x07
        ]
    );
}

// C.6.1
#[test]
fn decode_302() {
    assert_eq!(decode(&[0x64, 0x02]).unwrap(), b"302");
}

// C.6.2
#[test]
fn decode_307() {
    assert_eq!(decode(&[0x64, 0x0e, 0xff]).unwrap(), b"307");
}

// C.4.1
#[test]
fn decode_www_example_com() {
    assert_eq!(
        decode(&[
            0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90, 0xf4, 0xff
        ])
        .unwrap(),
        b"www.example.com"
    );
}

// C.6.1
#[test]
fn decode_https_www_example_com() {
    assert_eq!(
        decode(&[
            0x9d, 0x29, 0xad, 0x17, 0x18, 0x63, 0xc7, 0x8f, 0x0b, 0x97, 0xc8, 0xe9, 0xae, 0x82,
            0xae, 0x43, 0xd3
        ])
        .unwrap(),
        b"https://www.example.com"
    );
}

#[test]
fn decode_invalid_padding() {
    // "307" encodes to [0x64, 0x0e, 0xff]. The last byte is all
    // padding (EOS prefix). Replace 0xff with 0xfe to set the
    // lowest padding bit to 0.
    assert_eq!(
        decode(&[0x64, 0x0e, 0xfe]),
        Err(HuffmanError::InvalidPadding)
    );
}

#[test]
fn decode_empty() {
    assert_eq!(decode(&[]).unwrap(), b"");
}

#[test]
fn roundtrip_ascii() {
    let input = b"content-type: application/json";
    assert_eq!(decode(&encode(input)).unwrap(), input);
}

/// Cross-validate the nibble state machine against the bit-at-a-time
/// tree-walking decoder on random byte sequences.
///
/// Random bytes are generally *not* valid Huffman-encoded data, so most
/// inputs will produce errors (invalid padding, EOS in stream, etc.).
/// The important thing is that both decoders agree on the result.
#[test]
fn nibble_matches_bitwise_on_random_input() {
    let mut rng = fastrand::Rng::with_seed(0xdead_beef);
    for _ in 0..10_000 {
        let len = rng.usize(0..=64);
        let input: Vec<u8> = (0..len).map(|_| rng.u8(..)).collect();

        let nibble_result = decode(&input);
        let bitwise_result = decode_bitwise(&input);
        assert_eq!(
            nibble_result, bitwise_result,
            "mismatch on input ({len} bytes): {input:02x?}"
        );
    }
}

/// Same cross-validation but on valid Huffman data (encode random
/// ASCII, then decode). This exercises the happy path thoroughly.
#[test]
fn nibble_matches_bitwise_on_valid_encodings() {
    let mut rng = fastrand::Rng::with_seed(0xcafe_babe);
    for _ in 0..10_000 {
        let len = rng.usize(0..=128);
        let plaintext: Vec<u8> = (0..len).map(|_| rng.u8(0..=127)).collect();
        let encoded = encode(&plaintext);

        let nibble_result = decode(&encoded);
        let bitwise_result = decode_bitwise(&encoded);
        assert_eq!(
            nibble_result, bitwise_result,
            "mismatch on encoded form of {len}-byte plaintext"
        );

        // Also verify roundtrip correctness
        assert_eq!(
            nibble_result.unwrap(),
            plaintext,
            "roundtrip failed for {len}-byte plaintext"
        );
    }
}
