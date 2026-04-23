//! Single-pass HPACK encoder (RFC 7541 §6).
//!
//! [`HpackEncoder::encode`] walks the field section once, emitting wire bytes for each line
//! against the live dynamic-table state and applying any inserts inline. Reads (full-pair
//! match, name-only match, recent-pairs gate, observer hot-flag) and writes (table inserts,
//! recent-pairs remember, accumulator record) interleave naturally — a later line in the
//! same block sees the table state as updated by all earlier lines, which is exactly the
//! emission order HPACK's stateful decoder requires.
//!
//! ## Per-line decision
//!
//! 1. **§6.1 `IndexedStatic`** — full pair match in the static table. Emit the §6.1 prefix
//!    + index varint.
//! 2. **§6.1 `IndexedDynamic`** — full pair match in the (live) dynamic table. Emit §6.1 prefix +
//!    (61 + `dyn_idx`) varint.
//! 3. **`should_index` gate** — second sighting on this connection (via [`RecentPairs::seen`]) OR
//!    cross-connection observer hot-flag (via [`HeaderObserver::is_hot`]). Sensitive
//!    (uncacheable-value) names are excluded.
//! 4. **§6.2.1 `LiteralWithIncrementalIndexing`** when `should_index`. Emit prefix + name reference
//!    + value-string, then run the insert (§4.4 oversized-clears handled inside).
//! 5. **§6.2.2 `LiteralWithoutIndexing`** otherwise. Emit prefix + name reference + value-string.
//!
//! [`RecentPairs::seen`]: crate::headers::recent_pairs::RecentPairs::seen
//! [`HeaderObserver::is_hot`]: crate::headers::header_observer::HeaderObserver::is_hot

use super::{HpackEncoder, state::TableState};
use crate::headers::{
    entry_name::EntryName,
    field_section::{FieldLineValue, FieldSection},
    header_observer::HeaderObserver,
    hpack::static_table::static_table_lookup,
    huffman, integer_prefix,
    recent_pairs::RecentPairs,
    static_hit::StaticHit,
};

impl HpackEncoder {
    /// Encode `field_section` into `out` against this encoder's dynamic table, mutating the
    /// table inline as §6.2.1 inserts fire. The driver task calls this in pickup order;
    /// the dynamic-table mutation order matches the wire-emission order, as required by
    /// HPACK's stateful decoder.
    pub fn encode(&mut self, field_section: &FieldSection<'_>, out: &mut Vec<u8>) {
        for (name, value) in field_section.field_lines() {
            encode_line(&self.observer, &mut self.state, &name, value, out);
        }
    }
}

fn encode_line(
    observer: &HeaderObserver,
    state: &mut TableState,
    name: &EntryName<'_>,
    value: FieldLineValue<'_>,
    out: &mut Vec<u8>,
) {
    // Observer-accumulator bookkeeping. No effect on wire bytes; folded into the
    // shared cross-connection observer at this connection's Drop.
    if let Some(name_key) = name.name_key() {
        let static_value = if name.has_uncacheable_value() {
            None
        } else if let FieldLineValue::Static(s) = &value {
            Some(*s)
        } else {
            None
        };
        state.accum.record(name_key, static_value);
    }

    let value_bytes = value.as_bytes();
    let static_match = static_table_lookup(name, value_bytes);

    // 1. §6.1 IndexedStatic.
    if let StaticHit::Full(i) = static_match {
        emit_indexed(usize::from(i), out);
        return;
    }

    // 2. §6.1 IndexedDynamic — full pair match in the live dynamic table.
    let dyn_full_abs = state
        .by_name
        .get(name)
        .and_then(|ni| ni.by_value.get(value_bytes).copied());
    if let Some(abs_idx) = dyn_full_abs {
        // by_name's reverse index only references live entries, so live_dyn_idx_of
        // is guaranteed to succeed here.
        let dyn_idx = state
            .live_dyn_idx_of(abs_idx)
            .expect("by_name reverse index out of sync with live entries");
        emit_indexed(61 + dyn_idx, out);
        return;
    }

    // Pre-extract dyn-name lookup for use in the literal cases below.
    let dyn_name_abs = state.by_name.get(name).map(|ni| ni.latest_any);

    // 3. should-index gate. Never-indexed values are excluded from the recent-pairs ring and
    //    observer accounting, mirroring the static sensitive-headers list.
    let uncacheable = name.has_uncacheable_value() || never_indexed;
    let hash = (!uncacheable).then(|| RecentPairs::hash(name.as_bytes(), value_bytes));
    let observer_hot = !uncacheable && observer.is_hot(name, Some(&value));
    let should_index = hash.is_some_and(|h| state.recent_pairs.seen(h)) || observer_hot;

    if let Some(h) = hash {
        state.recent_pairs.remember(h);
    }

    if should_index {
        // 4. §6.2.1 LiteralWithIncrementalIndexing — emit then insert.
        let name_ref_idx = match static_match {
            StaticHit::Name(i) => Some(usize::from(i)),
            StaticHit::None => dyn_name_abs
                .and_then(|abs| state.live_dyn_idx_of(abs))
                .map(|d| 61 + d),
            StaticHit::Full(_) => unreachable!("step 1 returned for Full"),
        };
        if let Some(idx) = name_ref_idx {
            emit_literal_with_indexing_name_ref(idx, value_bytes, out);
        } else {
            emit_literal_with_indexing_literal_name(name.as_bytes(), value_bytes, out);
        }
        // Run the insert. §4.4 oversized-clears is handled inside.
        state.insert(name.reborrow(), value);
    } else {
        // 5. §6.2.2 LiteralWithoutIndexing.
        let name_ref_idx = match static_match {
            StaticHit::Name(i) => Some(usize::from(i)),
            StaticHit::None => dyn_name_abs
                .and_then(|abs| state.live_dyn_idx_of(abs))
                .map(|d| 61 + d),
            StaticHit::Full(_) => unreachable!("step 1 returned for Full"),
        };
        if let Some(idx) = name_ref_idx {
            emit_literal_without_indexing_name_ref(idx, value_bytes, out);
        } else {
            emit_literal_without_indexing_literal_name(name.as_bytes(), value_bytes, out);
        }
    }
}

// ---------- emit helpers ----------

/// §6.1 Indexed: `1xxxxxxx` with `index` packed into the low 7 bits + continuation.
fn emit_indexed(index: usize, out: &mut Vec<u8>) {
    let start = out.len();
    integer_prefix::encode_into(index, 7, out);
    out[start] |= 0b1000_0000;
}

/// §6.2.1 `LiteralWithIncrementalIndexing`, name reference: `01xxxxxx` + name index + value
/// string.
fn emit_literal_with_indexing_name_ref(name_index: usize, value: &[u8], out: &mut Vec<u8>) {
    let start = out.len();
    integer_prefix::encode_into(name_index, 6, out);
    out[start] |= 0b0100_0000;
    encode_string(value, out);
}

/// §6.2.1 `LiteralWithIncrementalIndexing`, literal name: `01000000` + name string + value
/// string.
fn emit_literal_with_indexing_literal_name(name: &[u8], value: &[u8], out: &mut Vec<u8>) {
    out.push(0b0100_0000);
    encode_string(name, out);
    encode_string(value, out);
}

/// §6.2.2 `LiteralWithoutIndexing`, name reference: `0000xxxx` + name index + value string.
fn emit_literal_without_indexing_name_ref(name_index: usize, value: &[u8], out: &mut Vec<u8>) {
    integer_prefix::encode_into(name_index, 4, out);
    encode_string(value, out);
}

/// §6.2.2 `LiteralWithoutIndexing`, literal name: `00000000` + name string + value string.
fn emit_literal_without_indexing_literal_name(name: &[u8], value: &[u8], out: &mut Vec<u8>) {
    out.push(0);
    encode_string(name, out);
    encode_string(value, out);
}

/// §5.2 string literal: H flag + 7-bit length prefix + bytes. Huffman-encodes when
/// strictly shorter.
fn encode_string(s: &[u8], buf: &mut Vec<u8>) {
    let start = buf.len();
    if let Some(huffman_len) = huffman::encoded_length_if_shorter(s) {
        integer_prefix::encode_into(huffman_len, 7, buf);
        buf[start] |= 0b1000_0000;
        huffman::encode_into(s, buf);
    } else {
        integer_prefix::encode_into(s.len(), 7, buf);
        buf.extend_from_slice(s);
    }
}
