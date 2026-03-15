use super::{HuffmanError, TABLE};

/// A node in the Huffman decoding tree.
#[derive(Debug, Clone, Copy)]
enum Node {
    /// Internal node with indices of (`zero_child`, `one_child`). Index 0
    /// is a sentinel meaning "no child allocated yet."
    Internal(u16, u16),
    /// Leaf node containing a decoded byte value (symbols 0-255).
    Leaf(u8),
    /// The end-of-string marker (symbol 256). Encountering this
    /// mid-stream is a decoding error.
    Eos,
}

/// 257 leaves + 256 internal nodes + 1 unused sentinel at index 0.
const TREE_CAPACITY: usize = 514;
const ROOT: u16 = 1;

/// Build the decoding tree at compile time from the encoding table.
///
/// Index 0 is reserved as a sentinel (never a valid node). The root
/// is at index 1. Internal nodes use 0 to mean "child not yet
/// allocated."
const fn build_decode_tree() -> [Node; TREE_CAPACITY] {
    let mut tree = [Node::Internal(0, 0); TREE_CAPACITY];
    let mut next_free = ROOT + 1;

    let mut sym = 0_u16;
    while sym < 257 {
        let (code, bit_length) = TABLE[sym as usize];
        let mut current = ROOT;
        let mut bit = bit_length;

        while bit > 0 {
            bit -= 1;
            let is_one = (code >> bit) & 1 == 1;

            let Node::Internal(zero, one) = tree[current as usize] else {
                panic!("walked into a leaf while inserting");
                // to debug: sym and bit are available here
            };

            let child = if is_one { one } else { zero };

            if child != 0 {
                // Already allocated — just follow it
                current = child;
            } else {
                // Allocate new node
                let new = next_free;
                next_free += 1;

                tree[new as usize] = if bit == 0 {
                    if sym < 256 {
                        #[allow(clippy::cast_possible_truncation)] // sym < 256
                        Node::Leaf(sym as u8)
                    } else {
                        Node::Eos
                    }
                } else {
                    Node::Internal(0, 0)
                };

                // Update parent's child pointer
                tree[current as usize] = if is_one {
                    Node::Internal(zero, new)
                } else {
                    Node::Internal(new, one)
                };

                current = new;
            }
        }

        sym += 1;
    }

    tree
}

const DECODE_TREE: [Node; TREE_CAPACITY] = build_decode_tree();

/// Huffman-decode a byte string per RFC 7541 §5.2.
pub(crate) fn decode(input: &[u8]) -> Result<Vec<u8>, HuffmanError> {
    let mut output = Vec::with_capacity(input.len()); // decoded is usually longer
    let mut bits_since_leaf = 0_u8;
    let mut padding_all_ones = true;

    let root_children = |node: Node| match node {
        Node::Internal(zero, one) => (zero, one),
        _ => unreachable!("root must be an internal node"),
    };

    let (mut zero, mut one) = root_children(DECODE_TREE[ROOT as usize]);

    for &byte in input {
        for bit_index in (0..8).rev() {
            let is_one = (byte >> bit_index) & 1 == 1;
            let child = if is_one { one } else { zero };
            bits_since_leaf += 1;
            padding_all_ones &= is_one;

            match DECODE_TREE[child as usize] {
                Node::Internal(z, o) => {
                    zero = z;
                    one = o;
                }
                Node::Leaf(decoded) => {
                    output.push(decoded);
                    (zero, one) = root_children(DECODE_TREE[ROOT as usize]);
                    bits_since_leaf = 0;
                    padding_all_ones = true;
                }
                Node::Eos => return Err(HuffmanError::EosInStream),
            }
        }
    }

    if bits_since_leaf > 7 {
        return Err(HuffmanError::PaddingTooLong);
    }

    if bits_since_leaf > 0 && !padding_all_ones {
        return Err(HuffmanError::InvalidPadding);
    }

    Ok(output)
}
