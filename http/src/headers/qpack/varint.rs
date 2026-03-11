/// Errors that can occur during variable-length integer decoding.
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone, Copy)]
pub enum VarIntError {
    /// The encoded integer overflows usize.
    #[error("encoded integer overflows usize")]
    Overflow,

    /// The input ended before the integer was fully decoded (more
    /// continuation bytes were expected).
    #[error("unexpected end of input")]
    UnexpectedEnd,
}

/// Decode a prefix-coded integer per RFC 7541 §5.1.
///
/// `prefix_size` is the number of bits in the first byte used for the
/// integer (the low bits). The remaining high bits belong to the
/// caller (flags, type indicators, etc.).
///
/// Returns the decoded value and the unconsumed remainder of the input.
pub(crate) fn decode(input: &[u8], prefix_size: u8) -> Result<(usize, &[u8]), VarIntError> {
    debug_assert!((1..=8).contains(&prefix_size));

    let [first, rest @ ..] = input else {
        return Err(VarIntError::UnexpectedEnd);
    };

    let prefix_mask = u8::MAX >> (8 - prefix_size);
    let mut value = usize::from(first & prefix_mask);

    if value < usize::from(prefix_mask) {
        return Ok((value, rest));
    }

    // Value continues in subsequent bytes
    let mut shift = 0_u32;
    for (i, &byte) in rest.iter().enumerate() {
        let payload = usize::from(byte & 0x7F);
        let increment = payload.checked_shl(shift).ok_or(VarIntError::Overflow)?;
        value = value.checked_add(increment).ok_or(VarIntError::Overflow)?;
        shift += 7;

        if byte & 0x80 == 0 {
            return Ok((value, &rest[i + 1..]));
        }
    }

    Err(VarIntError::UnexpectedEnd)
}

/// Encode a prefix-coded integer per RFC 7541 §5.1.
///
/// Returns the encoded bytes. The first byte contains the integer
/// value in its low `prefix_size` bits; the caller is responsible for
/// OR-ing in any flags in the high bits.
pub(crate) fn encode(value: usize, prefix_size: u8) -> Vec<u8> {
    debug_assert!((1..=8).contains(&prefix_size));

    let prefix_mask = usize::from(u8::MAX >> (8 - prefix_size));

    if value < prefix_mask {
        return vec![value as u8];
    }

    let mut output = vec![prefix_mask as u8];
    let mut remaining = value - prefix_mask;

    while remaining >= 128 {
        output.push((remaining % 128 + 128) as u8);
        remaining /= 128;
    }

    output.push(remaining as u8);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 7541 C.1.1: encoding 10 with 5-bit prefix
    #[test]
    fn decode_10_prefix_5() {
        let (value, rest) = decode(&[0b000_01010], 5).unwrap();
        assert_eq!(value, 10);
        assert!(rest.is_empty());
    }

    // RFC 7541 C.1.2: encoding 1337 with 5-bit prefix
    #[test]
    fn decode_1337_prefix_5() {
        let (value, rest) = decode(&[0b000_11111, 0b10011010, 0b00001010], 5).unwrap();
        assert_eq!(value, 1337);
        assert!(rest.is_empty());
    }

    // RFC 7541 C.1.3: encoding 42 with 8-bit prefix
    #[test]
    fn decode_42_prefix_8() {
        let (value, rest) = decode(&[42], 8).unwrap();
        assert_eq!(value, 42);
        assert!(rest.is_empty());
    }

    // High bits in first byte should be ignored
    #[test]
    fn decode_ignores_high_bits() {
        let (value, rest) = decode(&[0b111_01010], 5).unwrap();
        assert_eq!(value, 10);
        assert!(rest.is_empty());
    }

    // Trailing bytes should be returned unconsumed
    #[test]
    fn decode_returns_remainder() {
        let (value, rest) = decode(&[10, 0xAA, 0xBB], 8).unwrap();
        assert_eq!(value, 10);
        assert_eq!(rest, &[0xAA, 0xBB]);
    }

    #[test]
    fn decode_empty_input() {
        assert_eq!(decode(&[], 5), Err(VarIntError::UnexpectedEnd));
    }

    #[test]
    fn decode_truncated_continuation() {
        // Prefix bits all set (needs continuation) but no more bytes
        assert_eq!(decode(&[0b000_11111], 5), Err(VarIntError::UnexpectedEnd));
    }

    // RFC 7541 C.1.1: encoding 10 with 5-bit prefix
    #[test]
    fn encode_10_prefix_5() {
        assert_eq!(encode(10, 5), vec![0b0_1010]);
    }

    // RFC 7541 C.1.2: encoding 1337 with 5-bit prefix
    #[test]
    fn encode_1337_prefix_5() {
        assert_eq!(encode(1337, 5), vec![0b1_1111, 0b1001_1010, 0b0000_1010]);
    }

    // RFC 7541 C.1.3: encoding 42 with 8-bit prefix
    #[test]
    fn encode_42_prefix_8() {
        assert_eq!(encode(42, 8), vec![42]);
    }

    #[test]
    fn roundtrip_small() {
        for n in [1, 5, 7, 8] {
            for value in 0..300 {
                let encoded = encode(value, n);
                let (decoded, rest) = decode(&encoded, n).unwrap();
                assert_eq!(decoded, value, "prefix_size={n}, value={value}");
                assert!(rest.is_empty());
            }
        }
    }

    #[test]
    fn roundtrip_large() {
        for &value in &[
            0,
            1,
            126,
            127,
            128,
            255,
            256,
            16383,
            16384,
            1_000_000,
            usize::MAX >> 1,
        ] {
            let encoded = encode(value, 5);
            let (decoded, rest) = decode(&encoded, 5).unwrap();
            assert_eq!(decoded, value);
            assert!(rest.is_empty());
        }
    }
}
