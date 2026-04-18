use super::TABLE;

/// Huffman-encode a byte string per RFC 7541 §5.2.
pub(in crate::headers) fn encode(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len()); // encoded is usually shorter
    let mut accumulator: u64 = 0;
    let mut bits_used: u8 = 0;

    for &byte in input {
        let (code, bit_length) = TABLE[byte as usize];
        accumulator |= u64::from(code) << (64 - bits_used - bit_length);
        bits_used += bit_length;

        while bits_used >= 8 {
            output.push((accumulator >> 56) as u8);
            accumulator <<= 8;
            bits_used -= 8;
        }
    }

    // Pad remaining bits with 1s (EOS prefix) and emit final byte
    if bits_used > 0 {
        accumulator |= 0xFF_u64 << (56 - bits_used);
        output.push((accumulator >> 56) as u8);
    }

    output
}
