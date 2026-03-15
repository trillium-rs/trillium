//! QUIC Variable-length integer coding
//!
//! Caveat usor: This is only public under cfg("unstable"), and is, as it says on the tin,
//! semver-unstable.
//!
//! See [RFC 9000 §16](https://datatracker.ietf.org/doc/html/rfc9000#section-16) for specification

/// Errors that can occur during QUIC variable-length integer decoding.
#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
pub enum QuicVarIntError {
    /// The input ended before the integer was fully decoded.
    #[error("unexpected end of input")]
    UnexpectedEnd,

    /// The varint decoded successfully but the value has no mapping in the target type.
    #[error("unknown varint value {value}")]
    UnknownValue {
        /// The raw decoded u64.
        value: u64,
        /// The number of bytes parsed
        bytes: usize,
    },
}

/// Decode a QUIC variable-length integer per [RFC 9000
/// §16](https://datatracker.ietf.org/doc/html/rfc9000#section-16).
///
/// The 2-bit prefix of the first byte encodes the length:
/// `00` = 1 byte (6-bit value), `01` = 2 bytes (14-bit),
/// `10` = 4 bytes (30-bit), `11` = 8 bytes (62-bit).
///
/// The decoded varint is converted to `T` via `TryFrom<u64>`.
/// Returns the decoded value and the unconsumed remainder of the input.
///
/// # Errors
///
/// Returns a [`QuicVarIntError`] if:
/// - input does not contain a full varint
/// - we were not able to convert the decoded u64 into the provided T
pub fn decode<T: TryFrom<u64>>(input: &[u8]) -> Result<(T, usize), QuicVarIntError> {
    let [first, ..] = input else {
        return Err(QuicVarIntError::UnexpectedEnd);
    };

    let prefix = first >> 6;
    let len = 1_usize << prefix; // 1, 2, 4, or 8

    if input.len() < len {
        return Err(QuicVarIntError::UnexpectedEnd);
    }

    let mut value = u64::from(first & 0x3F);
    for &byte in &input[1..len] {
        value = value << 8 | u64::from(byte);
    }

    let converted =
        T::try_from(value).map_err(|_| QuicVarIntError::UnknownValue { value, bytes: len })?;

    Ok((converted, len))
}

/// The number of bytes needed to encode `value` as a QUIC varint.
///
/// # Panics
///
/// Panics if the value exceeds 2^62 - 1.
pub fn encoded_len(value: impl Into<u64>) -> usize {
    let value = value.into();
    if value < (1 << 6) {
        1
    } else if value < (1 << 14) {
        2
    } else if value < (1 << 30) {
        4
    } else {
        assert!(value < (1 << 62), "QUIC varint overflow: {value}");
        8
    }
}

/// Encode a QUIC variable-length integer into a byte slice.
///
/// Returns the number of bytes written, or `None` if the value exceeds
/// 2^62 - 1 or `buf` is too small (caller should check with
/// [`encoded_len`] first).
pub fn encode(value: impl Into<u64>, buf: &mut [u8]) -> Option<usize> {
    let value = value.into();
    let bytes = value.to_be_bytes();

    // The 2-bit length prefix and the byte range within `bytes` are determined
    // by the magnitude of the value.  After copying the relevant big-endian
    // tail into `buf`, we OR the prefix into the first byte.
    let (prefix, start) = if value < (1 << 6) {
        (0x00_u8, 7)
    } else if value < (1 << 14) {
        (0x40, 6)
    } else if value < (1 << 30) {
        (0x80, 4)
    } else if value < (1 << 62) {
        (0xC0, 0)
    } else {
        return None; // value exceeds 2^62 - 1
    };

    let dest = buf.get_mut(..8 - start)?;
    dest.copy_from_slice(&bytes[start..]);
    dest[0] |= prefix;
    Some(dest.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h3::frame::FrameType;

    #[test]
    fn decode_one_byte() {
        let (value, bytes_read) = decode::<u64>(&[0x25]).unwrap();
        assert_eq!(value, 37);
        assert_eq!(bytes_read, 1);
    }

    #[test]
    fn decode_two_bytes() {
        // RFC 9000 §16 example: 0x7bbd = 15293
        let (value, bytes_read) = decode::<u64>(&[0x7b, 0xbd]).unwrap();
        assert_eq!(value, 15293);
        assert_eq!(bytes_read, 2);
    }

    #[test]
    fn decode_four_bytes() {
        // RFC 9000 §16 example: 0x9d7f3e7d = 494878333
        let (value, bytes_read) = decode::<u64>(&[0x9d, 0x7f, 0x3e, 0x7d]).unwrap();
        assert_eq!(value, 494_878_333);
        assert_eq!(bytes_read, 4);
    }

    #[test]
    fn decode_eight_bytes() {
        // RFC 9000 §16 example: 0xc2197c5eff14e88c = 151288809941952652
        let (value, bytes_read) =
            decode::<u64>(&[0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c]).unwrap();
        assert_eq!(value, 151_288_809_941_952_652);
        assert_eq!(bytes_read, 8);
    }

    #[test]
    fn decode_returns_bytes_read() {
        let (value, bytes_read) = decode::<u8>(&[0x25, 0xAA]).unwrap();
        assert_eq!(value, 37);
        assert_eq!(bytes_read, 1);
    }

    #[test]
    fn decode_empty() {
        assert_eq!(decode::<u64>(&[]), Err(QuicVarIntError::UnexpectedEnd));
    }

    #[test]
    fn decode_truncated() {
        // Two-byte encoding but only one byte provided
        assert_eq!(decode::<u64>(&[0x40]), Err(QuicVarIntError::UnexpectedEnd));
    }

    #[test]
    fn decode_value_out_of_range_for_u8() {
        // Encodes 15293 — valid varint but too large for u8
        assert_eq!(
            decode::<u8>(&[0x7b, 0xbd]),
            Err(QuicVarIntError::UnknownValue {
                value: 15293,
                bytes: 2
            })
        );
    }

    #[test]
    fn decode_into_frame_type() {
        // 0x01 = FrameType::Headers
        let (frame_type, bytes_read) = decode::<FrameType>(&[0x01]).unwrap();
        assert_eq!(frame_type, FrameType::Headers);
        assert_eq!(bytes_read, 1);
    }

    #[test]
    fn decode_unknown_frame_type() {
        // 0x09 is not a known frame type
        assert_eq!(
            decode::<FrameType>(&[0x09]),
            Err(QuicVarIntError::UnknownValue {
                value: 0x09,
                bytes: 1
            })
        );
    }

    #[test]
    fn roundtrip() {
        for value in [
            0u64,
            1,
            63,
            64,
            16_383,
            16_384,
            1_073_741_823,
            1_073_741_824,
            (1u64 << 62) - 1,
        ] {
            let mut buf = vec![0; 256];
            let bytes_written = encode(value, &mut buf).unwrap();
            let (decoded, bytes_read) = decode::<u64>(&buf).unwrap();
            assert_eq!(decoded, value, "roundtrip failed for {value}");
            assert_eq!(bytes_read, bytes_written);
        }
    }

    #[test]
    fn encode_uses_smallest_encoding() {
        let mut buf = vec![0; 256];
        assert_eq!(Some(1), encode(0u8, &mut buf));
        assert_eq!(Some(1), encode(63u8, &mut buf));
        assert_eq!(Some(2), encode(64u16, &mut buf));
        assert_eq!(Some(4), encode(16_384u32, &mut buf));
        assert_eq!(Some(8), encode(1_073_741_824u64, &mut buf));
    }
}
