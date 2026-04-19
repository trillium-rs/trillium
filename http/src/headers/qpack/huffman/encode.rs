use super::TABLE;

/// Huffman-encoded length of `input` per RFC 7541 §5.2, but only if the encoded form would
/// be *strictly* shorter than the raw input. Returns `None` when Huffman would be equal or
/// longer, short-circuiting the bit-count accumulation as soon as that outcome is decided.
///
/// Strictly-shorter means `ceil(total_bits / 8) < input.len()`, which holds iff
/// `total_bits <= 8 * (input.len() - 1)`. The loop bails the first time the running bit
/// count exceeds that budget. The empty input and single-byte inputs can never satisfy the
/// inequality (shortest Huffman code is 5 bits, padded to 1 byte), so they short-circuit
/// before the loop via the `checked_sub`.
///
/// Used by [`super::super::instruction::encode_string`] to pick Huffman vs raw without
/// materializing the encoded form just to compare lengths.
pub(in crate::headers) fn encoded_length_if_shorter(input: &[u8]) -> Option<usize> {
    // `input.len() * 8` can't overflow usize for any real input (bounded by the wire
    // length limits enforced at parse time), but checked math keeps us out of UB territory.
    let budget_bits = input.len().checked_mul(8).and_then(|x| x.checked_sub(8))?;
    let mut total_bits: usize = 0;
    for &byte in input {
        total_bits += usize::from(TABLE[byte as usize].1);
        if total_bits > budget_bits {
            return None;
        }
    }
    Some(total_bits.div_ceil(8))
}

/// Huffman-encode `input` per RFC 7541 §5.2, appending the encoded bytes to `buf`.
pub(in crate::headers) fn encode_into(input: &[u8], buf: &mut Vec<u8>) {
    let mut accumulator: u64 = 0;
    let mut bits_used: u8 = 0;

    for &byte in input {
        let (code, bit_length) = TABLE[byte as usize];
        accumulator |= u64::from(code) << (64 - bits_used - bit_length);
        bits_used += bit_length;

        while bits_used >= 8 {
            buf.push((accumulator >> 56) as u8);
            accumulator <<= 8;
            bits_used -= 8;
        }
    }

    // Pad remaining bits with 1s (EOS prefix) and emit final byte
    if bits_used > 0 {
        accumulator |= 0xFF_u64 << (56 - bits_used);
        buf.push((accumulator >> 56) as u8);
    }
}
