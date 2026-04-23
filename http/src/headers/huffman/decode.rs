use super::{HuffmanError, TABLE};

// ---- Compile-time tree (used only during table construction) ----

#[derive(Clone, Copy)]
enum Node {
    /// Internal node: (`zero_child`, `one_child`). Index 0 is a sentinel
    /// meaning "no child allocated yet."
    Internal(u16, u16),
    /// Leaf node containing a decoded byte value (symbols 0–255).
    Leaf(u8),
    /// End-of-string marker (symbol 256).
    Eos,
}

/// 257 leaves + 256 internal nodes + 1 unused sentinel at index 0.
const TREE_CAPACITY: usize = 514;
const ROOT: u16 = 1;

/// A full binary tree with 257 leaves has exactly 256 internal nodes.
const NUM_STATES: usize = 256;

/// Build a binary Huffman tree from the encoding table at compile time.
///
/// Index 0 is a sentinel (never valid). The root is at index 1. Internal
/// nodes use 0 to mean "child not yet allocated."
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
            };

            let child = if is_one { one } else { zero };

            if child != 0 {
                current = child;
            } else {
                let new = next_free;
                next_free += 1;

                #[allow(clippy::cast_possible_truncation)] // sym < 256
                {
                    tree[new as usize] = if bit == 0 {
                        if sym < 256 {
                            Node::Leaf(sym as u8)
                        } else {
                            Node::Eos
                        }
                    } else {
                        Node::Internal(0, 0)
                    };
                }

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

// ---- Nibble-indexed state machine ----

/// Entry in the nibble-indexed Huffman decode table.
///
/// For each `(state, nibble)` pair, this records the outcome of walking
/// the Huffman tree for 4 bits from that state.
#[derive(Clone, Copy)]
struct NibbleEntry {
    /// Decoder state after processing this nibble.
    next_state: u8,
    /// Decoded byte value (valid only when `emit` is true).
    symbol: u8,
    /// Whether a complete Huffman code was decoded in this nibble.
    emit: bool,
    /// Whether the EOS marker was encountered (decoding error per RFC 7541 §5.2).
    eos: bool,
}

/// Precomputed nibble-indexed decode table with end-of-input metadata.
///
/// Processes 4 bits per lookup, requiring exactly 2 lookups per input
/// byte. At most one symbol can be decoded per nibble because the
/// shortest Huffman code in the HPACK table is 5 bits.
struct NibbleDecoder {
    /// `table[state][nibble]` → decode outcome for that 4-bit input.
    table: [[NibbleEntry; 16]; NUM_STATES],

    /// Depth of each state in the Huffman tree (= bits consumed since
    /// the last decoded symbol). Used for end-of-input padding validation.
    depth: [u8; NUM_STATES],

    /// Whether each state lies on the all-ones path from the root.
    /// Combined with `depth ≤ 7`, determines whether end-of-input
    /// padding is valid per RFC 7541 §5.2.
    padding_valid: [bool; NUM_STATES],
}

#[allow(clippy::too_many_lines)] // single const construction with sequential phases
const fn build_nibble_decoder() -> NibbleDecoder {
    let tree = build_decode_tree();

    // --- Assign sequential state indices to internal nodes ---
    //
    // Tree nodes are allocated with monotonically increasing indices
    // (parents before children), so scanning in index order assigns
    // state 0 to the root.
    let mut node_to_state = [0_u8; TREE_CAPACITY];
    let mut state_to_node = [0_u16; NUM_STATES];
    let mut num_states = 0_usize;

    let mut i = 1_usize;
    while i < TREE_CAPACITY {
        if let Node::Internal(_, _) = tree[i] {
            #[allow(clippy::cast_possible_truncation)]
            {
                node_to_state[i] = num_states as u8;
                state_to_node[num_states] = i as u16;
            }
            num_states += 1;
        }
        i += 1;
    }
    assert!(num_states == NUM_STATES, "expected 256 internal nodes");

    // --- Compute tree depth for each node ---
    //
    // Because parents have lower indices than children, a forward scan
    // propagates depths correctly.
    let mut node_depth = [0_u8; TREE_CAPACITY];
    let mut n = 1_usize;
    while n < TREE_CAPACITY {
        if let Node::Internal(zero, one) = tree[n] {
            let d = node_depth[n] + 1;
            if zero != 0 {
                node_depth[zero as usize] = d;
            }
            if one != 0 {
                node_depth[one as usize] = d;
            }
        }
        n += 1;
    }

    let mut depth = [0_u8; NUM_STATES];
    let mut s = 0_usize;
    while s < NUM_STATES {
        depth[s] = node_depth[state_to_node[s] as usize];
        s += 1;
    }

    // --- Mark the all-ones path from root as valid padding ---
    //
    // Valid HPACK padding is a prefix of the EOS code (all 1-bits).
    // Walk from the root following only the `one` child at each level;
    // every internal node on this path is a valid padding state.
    let mut padding_valid = [false; NUM_STATES];
    padding_valid[node_to_state[ROOT as usize] as usize] = true;

    let mut current = ROOT as usize;
    while let Node::Internal(_, one) = tree[current] {
        if one == 0 {
            break;
        }
        match tree[one as usize] {
            Node::Internal(_, _) => {
                padding_valid[node_to_state[one as usize] as usize] = true;
                current = one as usize;
            }
            _ => break,
        }
    }

    // --- Build the nibble lookup table ---
    let empty = NibbleEntry {
        next_state: 0,
        symbol: 0,
        emit: false,
        eos: false,
    };
    let mut table = [[empty; 16]; NUM_STATES];

    let mut state = 0_usize;
    while state < NUM_STATES {
        let start_node = state_to_node[state] as usize;
        let mut nibble = 0_u8;
        while nibble < 16 {
            let mut current = start_node;
            let mut symbol = 0_u8;
            let mut emit = false;
            let mut eos = false;

            let mut bit = 4_u8;
            while bit > 0 && !eos {
                bit -= 1;
                let is_one = (nibble >> bit) & 1 == 1;

                let Node::Internal(zero, one) = tree[current] else {
                    panic!("expected internal node during nibble walk");
                };

                let child = if is_one { one } else { zero };

                match tree[child as usize] {
                    Node::Internal(_, _) => {
                        current = child as usize;
                    }
                    Node::Leaf(sym) => {
                        symbol = sym;
                        emit = true;
                        current = ROOT as usize;
                    }
                    Node::Eos => {
                        eos = true;
                    }
                }
            }

            table[state][nibble as usize] = NibbleEntry {
                next_state: node_to_state[current],
                symbol,
                emit,
                eos,
            };

            nibble += 1;
        }
        state += 1;
    }

    NibbleDecoder {
        table,
        depth,
        padding_valid,
    }
}

static DECODER: NibbleDecoder = build_nibble_decoder();

/// Bit-at-a-time tree-walking decoder, retained as a test oracle for
/// cross-validating the nibble state machine.
#[cfg(test)]
pub(super) fn decode_bitwise(input: &[u8]) -> Result<Vec<u8>, HuffmanError> {
    static TREE: [Node; TREE_CAPACITY] = build_decode_tree();

    let root_children = |node: Node| match node {
        Node::Internal(zero, one) => (zero, one),
        _ => unreachable!("root must be an internal node"),
    };

    let mut output = Vec::with_capacity(input.len());
    let mut bits_since_leaf = 0_u8;
    let mut padding_all_ones = true;
    let (mut zero, mut one) = root_children(TREE[ROOT as usize]);

    for &byte in input {
        for bit_index in (0..8).rev() {
            let is_one = (byte >> bit_index) & 1 == 1;
            let child = if is_one { one } else { zero };
            bits_since_leaf += 1;
            padding_all_ones &= is_one;

            match TREE[child as usize] {
                Node::Internal(z, o) => {
                    zero = z;
                    one = o;
                }
                Node::Leaf(decoded) => {
                    output.push(decoded);
                    (zero, one) = root_children(TREE[ROOT as usize]);
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

/// Huffman-decode a byte string per RFC 7541 §5.2.
///
/// Uses a precomputed nibble-indexed state machine: each input byte
/// requires exactly two table lookups (high nibble, low nibble),
/// processing 4 bits per step instead of the naive 1 bit at a time.
pub(in crate::headers) fn decode(input: &[u8]) -> Result<Vec<u8>, HuffmanError> {
    let mut output = Vec::with_capacity(input.len());
    let mut state = 0_usize; // state 0 = root

    for &byte in input {
        let entry = DECODER.table[state][(byte >> 4) as usize];
        if entry.eos {
            return Err(HuffmanError::EosInStream);
        }
        if entry.emit {
            output.push(entry.symbol);
        }
        state = entry.next_state as usize;

        let entry = DECODER.table[state][(byte & 0xF) as usize];
        if entry.eos {
            return Err(HuffmanError::EosInStream);
        }
        if entry.emit {
            output.push(entry.symbol);
        }
        state = entry.next_state as usize;
    }

    let depth = DECODER.depth[state];
    if depth > 7 {
        return Err(HuffmanError::PaddingTooLong);
    }
    if depth > 0 && !DECODER.padding_valid[state] {
        return Err(HuffmanError::InvalidPadding);
    }

    Ok(output)
}
