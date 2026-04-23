//! Reference-encoder `.out` file parser for the corpus comparison dashboard.
//!
//! The QPACK offline-interop corpus ships precomputed encoder outputs under
//! `tests/qifs/encoded/qpack-06/{encoder}/{stem}.out.{cap}.{blocked}.{variant}`. Each
//! file is a sequence of records: `[8-byte stream_id][4-byte length][length bytes of
//! payload]`. `stream_id == 0` means encoder-stream instructions; any other stream_id is
//! a header block for that stream. This module walks those records and classifies each
//! instruction into a [`WireHistogram`] that can be compared, bucket for bucket, against
//! a histogram derived from our own encoder output.
//!
//! Development-time scaffolding. The corpus test is the only caller.
//!
//! ## Why wire-level rather than policy-level comparison
//!
//! Different encoders can arrive at identical wire output via very different internal
//! decisions, and any internal counters we might keep on our side are not derivable from
//! a reference encoder's output. A wire-level histogram is the common denominator —
//! what any two encoders produce for the same input, in the same shape.
//!
//! ## Wire-shape divergences to expect
//!
//! Even encoders with identical policy can emit differently-shaped bytes:
//!
//! - **Pre-base vs post-base indexing for newly-inserted entries.** Ls-qpack emits
//!   `IndexedPostBase`/`LiteralPostBaseNameRef` for references to entries inserted by the current
//!   header block; we emit `IndexedDynamic`/`LiteralDynamicNameRef` (pre-base with `delta_base =
//!   0`). Both carry the same semantic and cost a single byte for the common case. See the "total
//!   dynamic refs" derived line in [`WireHistogram::print_comparison`].
//! - **Huffman-or-raw picks for strings** can differ between encoders on short or high-entropy
//!   values, independently of any policy difference.
//!
//! The comparison report flags buckets that differ by more than 2× and annotates those
//! that tend to alias across encoders with the divergence note.

#![cfg(test)]

use super::instruction::{
    encoder::{EncoderInstruction, parse as parse_encoder_instr},
    field_section::{FieldLineInstruction, FieldSectionPrefix},
};
use futures_lite::future;
use std::path::Path;

/// Wire-visible instruction histogram for a sequence of encoded bytes. Populated either
/// from a reference `.out` file via [`histogram_from_out_file`] or from our own
/// encoded bytes via [`classify_encoder_stream`] + [`classify_header_block`].
#[derive(Debug, Default, Clone, Copy)]
pub(in crate::headers::qpack) struct WireHistogram {
    // --- encoder-stream §3.2 instructions ---
    pub(in crate::headers::qpack) set_capacity: u64,
    pub(in crate::headers::qpack) insert_literal_name: u64,
    pub(in crate::headers::qpack) insert_static_name_ref: u64,
    pub(in crate::headers::qpack) insert_dynamic_name_ref: u64,
    pub(in crate::headers::qpack) duplicate: u64,
    // --- header-block §4.5 instructions ---
    pub(in crate::headers::qpack) indexed_static: u64,
    pub(in crate::headers::qpack) indexed_dynamic_pre_base: u64,
    pub(in crate::headers::qpack) indexed_post_base: u64,
    pub(in crate::headers::qpack) literal_static_name_ref: u64,
    pub(in crate::headers::qpack) literal_dynamic_name_ref: u64,
    pub(in crate::headers::qpack) literal_post_base_name_ref: u64,
    pub(in crate::headers::qpack) literal_literal_name: u64,
    // --- aggregates ---
    /// Number of non-zero stream_id records seen (header blocks). Zero for our own
    /// aggregated classification; the caller bumps this as it processes each section.
    pub(in crate::headers::qpack) n_sections: u64,
    /// Bytes spent in header-block records.
    pub(in crate::headers::qpack) section_bytes: u64,
    /// Bytes spent in encoder-stream records (stream_id = 0).
    pub(in crate::headers::qpack) encoder_stream_bytes: u64,
}

impl WireHistogram {
    /// Sum `other` into `self`. For aggregation across qif files.
    pub(in crate::headers::qpack) fn add(&mut self, other: &Self) {
        self.set_capacity += other.set_capacity;
        self.insert_literal_name += other.insert_literal_name;
        self.insert_static_name_ref += other.insert_static_name_ref;
        self.insert_dynamic_name_ref += other.insert_dynamic_name_ref;
        self.duplicate += other.duplicate;
        self.indexed_static += other.indexed_static;
        self.indexed_dynamic_pre_base += other.indexed_dynamic_pre_base;
        self.indexed_post_base += other.indexed_post_base;
        self.literal_static_name_ref += other.literal_static_name_ref;
        self.literal_dynamic_name_ref += other.literal_dynamic_name_ref;
        self.literal_post_base_name_ref += other.literal_post_base_name_ref;
        self.literal_literal_name += other.literal_literal_name;
        self.n_sections += other.n_sections;
        self.section_bytes += other.section_bytes;
        self.encoder_stream_bytes += other.encoder_stream_bytes;
    }

    /// Convenience: total encoder-stream inserts of any flavor (literal name, static
    /// name ref, dynamic name ref). Excludes Duplicate and SetCapacity.
    pub(in crate::headers::qpack) fn inserts_total(&self) -> u64 {
        self.insert_literal_name + self.insert_static_name_ref + self.insert_dynamic_name_ref
    }

    /// Convenience: total indexed-dynamic field lines (pre-base + post-base). These
    /// alias across encoders — collapsing them makes cross-encoder comparison meaningful.
    pub(in crate::headers::qpack) fn indexed_dynamic_total(&self) -> u64 {
        self.indexed_dynamic_pre_base + self.indexed_post_base
    }

    /// Convenience: total literal-dynamic-name-ref field lines (pre-base + post-base).
    pub(in crate::headers::qpack) fn literal_dyn_name_total(&self) -> u64 {
        self.literal_dynamic_name_ref + self.literal_post_base_name_ref
    }
}

/// Walk an `.out` file and classify every instruction inside it. Returns `Ok(None)` if
/// the file does not exist (common — not every reference encoder publishes outputs at
/// every config); returns `Err` only on a genuine I/O error.
pub(in crate::headers::qpack) fn histogram_from_out_file(
    path: &Path,
) -> std::io::Result<Option<WireHistogram>> {
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut hist = WireHistogram::default();
    let mut pos = 0usize;
    while pos + 12 <= data.len() {
        let stream_id = u64::from_be_bytes(data[pos..pos + 8].try_into().unwrap());
        let length = u32::from_be_bytes(data[pos + 8..pos + 12].try_into().unwrap()) as usize;
        pos += 12;
        if pos + length > data.len() {
            // Truncated record — stop but keep what we have; mirrors
            // `sum_reference_out_bytes`'s resilient walk.
            break;
        }
        let payload = &data[pos..pos + length];
        if stream_id == 0 {
            hist.encoder_stream_bytes += length as u64;
            classify_encoder_stream(payload, &mut hist);
        } else {
            hist.n_sections += 1;
            hist.section_bytes += length as u64;
            classify_header_block(payload, &mut hist);
        }
        pos += length;
    }
    Ok(Some(hist))
}

/// Parse a buffer of encoder-stream bytes (`stream_id == 0` payload or our own
/// `enc_ops` accumulator) and bump the matching counters in `hist`. On a parse error,
/// stops silently — reference files are trusted, and our own encoder is already verified
/// by the correctness assertions in the corpus test.
pub(in crate::headers::qpack) fn classify_encoder_stream(bytes: &[u8], hist: &mut WireHistogram) {
    let mut stream = bytes;
    // `max_entry_size: usize::MAX` — we're reading for classification, not enforcing
    // a policy limit. The length prefix on each string is the only bound that matters.
    while let Ok(Some(instr)) = future::block_on(parse_encoder_instr(usize::MAX, &mut stream)) {
        match instr {
            EncoderInstruction::SetCapacity(_) => hist.set_capacity += 1,
            EncoderInstruction::InsertWithLiteralName { .. } => hist.insert_literal_name += 1,
            EncoderInstruction::InsertWithStaticNameRef { .. } => {
                hist.insert_static_name_ref += 1;
            }
            EncoderInstruction::InsertWithDynamicNameRef { .. } => {
                hist.insert_dynamic_name_ref += 1;
            }
            EncoderInstruction::Duplicate { .. } => hist.duplicate += 1,
        }
    }
}

/// Parse a buffer of header-block bytes (stripping the §4.5.1 prefix internally) and bump
/// the matching counters in `hist`. Stops on the first parse error.
pub(in crate::headers::qpack) fn classify_header_block(bytes: &[u8], hist: &mut WireHistogram) {
    let Ok((_prefix, mut rest)) = FieldSectionPrefix::parse(bytes) else {
        return;
    };
    while !rest.is_empty() {
        match FieldLineInstruction::parse(rest) {
            Ok((instr, remaining)) => {
                match instr {
                    FieldLineInstruction::IndexedStatic { .. } => hist.indexed_static += 1,
                    FieldLineInstruction::IndexedDynamic { .. } => {
                        hist.indexed_dynamic_pre_base += 1;
                    }
                    FieldLineInstruction::IndexedPostBase { .. } => hist.indexed_post_base += 1,
                    FieldLineInstruction::LiteralStaticNameRef { .. } => {
                        hist.literal_static_name_ref += 1;
                    }
                    FieldLineInstruction::LiteralDynamicNameRef { .. } => {
                        hist.literal_dynamic_name_ref += 1;
                    }
                    FieldLineInstruction::LiteralPostBaseNameRef { .. } => {
                        hist.literal_post_base_name_ref += 1;
                    }
                    FieldLineInstruction::LiteralLiteralName { .. } => {
                        hist.literal_literal_name += 1;
                    }
                }
                rest = remaining;
            }
            Err(_) => break,
        }
    }
}

// =====================================================================================
// Per-group ASCII dump — used by the corpus test's in-situ divergence inspection when
// `QPACK_ENCODER_DUMP=substring` is set. Writes one file per (qif, config) into
// `target/qpack-dump/`. Tears out with the rest of the scaffolding.
// =====================================================================================

/// One group's worth of `.out` records: the encoder-stream records that appeared
/// between the previous group and this group's header block, plus the header-block bytes
/// themselves. Groups are ordered as written on the wire.
pub(in crate::headers::qpack) struct OutGroup {
    /// Zero or more encoder-stream record payloads (stream_id = 0). Concatenated, they
    /// form the encoder-stream instruction sequence this group prepared.
    pub(in crate::headers::qpack) enc_stream: Vec<Vec<u8>>,
    /// The header-block record payload (stream_id = this group's stream_id). Includes
    /// the §4.5.1 prefix.
    pub(in crate::headers::qpack) header_block: Vec<u8>,
}

/// Parse a reference `.out` file into an ordered list of groups. Every non-zero stream_id
/// record becomes one group; preceding zero stream_id records are attached to it.
/// Returns `None` if the file does not exist.
pub(in crate::headers::qpack) fn parse_out_groups(path: &Path) -> Option<Vec<OutGroup>> {
    let data = std::fs::read(path).ok()?;
    let mut groups = Vec::new();
    let mut pending_enc: Vec<Vec<u8>> = Vec::new();
    let mut pos = 0usize;
    while pos + 12 <= data.len() {
        let stream_id = u64::from_be_bytes(data[pos..pos + 8].try_into().unwrap());
        let length = u32::from_be_bytes(data[pos + 8..pos + 12].try_into().unwrap()) as usize;
        pos += 12;
        if pos + length > data.len() {
            break;
        }
        let payload = data[pos..pos + length].to_vec();
        if stream_id == 0 {
            pending_enc.push(payload);
        } else {
            groups.push(OutGroup {
                enc_stream: std::mem::take(&mut pending_enc),
                header_block: payload,
            });
        }
        pos += length;
    }
    Some(groups)
}

/// Render one parsed encoder-stream instruction as a compact single line. Not a `Display`
/// impl on the type — kept local to the dev-scaffolding module so nothing leaks into
/// production interfaces.
pub(in crate::headers::qpack) fn render_encoder_instruction(i: &EncoderInstruction) -> String {
    use EncoderInstruction::*;
    match i {
        SetCapacity(n) => format!("SET_CAPACITY {n}"),
        InsertWithLiteralName { name, value } => format!(
            "INS_LITERAL_NAME   {:<30} = {}",
            render_name(name),
            render_value(value),
        ),
        InsertWithStaticNameRef { name_index, value } => format!(
            "INS_STATIC_NAME    static[{name_index}] = {}",
            render_value(value),
        ),
        InsertWithDynamicNameRef {
            relative_index,
            value,
        } => format!(
            "INS_DYN_NAME       dyn[rel={relative_index}] = {}",
            render_value(value),
        ),
        Duplicate { relative_index } => format!("DUP                dyn[rel={relative_index}]"),
    }
}

/// Render one parsed field-line instruction as a compact single line.
pub(in crate::headers::qpack) fn render_field_line(i: &FieldLineInstruction<'_>) -> String {
    use FieldLineInstruction::*;
    match i {
        IndexedStatic { index } => format!("IDX_STATIC         static[{index}]"),
        IndexedDynamic { relative_index } => {
            format!("IDX_DYN_PRE_BASE   dyn[rel={relative_index}]")
        }
        IndexedPostBase { post_base_index } => {
            format!("IDX_DYN_POST_BASE  dyn[post+{post_base_index}]")
        }
        LiteralStaticNameRef {
            name_index, value, ..
        } => format!(
            "LIT_STATIC_NAME    static[{name_index}] = {}",
            render_value(value.as_bytes()),
        ),
        LiteralDynamicNameRef {
            relative_index,
            value,
            ..
        } => format!(
            "LIT_DYN_PRE_BASE   dyn[rel={relative_index}] = {}",
            render_value(value.as_bytes()),
        ),
        LiteralPostBaseNameRef {
            post_base_index,
            value,
            ..
        } => format!(
            "LIT_DYN_POST_BASE  dyn[post+{post_base_index}] = {}",
            render_value(value.as_bytes()),
        ),
        LiteralLiteralName { name, value, .. } => format!(
            "LIT_LITERAL_NAME   {:<30} = {}",
            render_name(name),
            render_value(value.as_bytes()),
        ),
    }
}

fn render_name(n: &crate::headers::entry_name::EntryName<'_>) -> String {
    render_value(n.as_bytes())
}

/// Truncate and escape a byte string for inclusion in a dump line.
fn render_value(bytes: &[u8]) -> String {
    const LIMIT: usize = 60;
    let mut out = String::with_capacity(bytes.len().min(LIMIT) + 2);
    out.push('"');
    for (i, &b) in bytes.iter().enumerate() {
        if i >= LIMIT {
            out.push_str("...");
            break;
        }
        match b {
            0x20..=0x7e if b != b'"' && b != b'\\' => out.push(b as char),
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\\""),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            _ => out.push_str(&format!("\\x{:02x}", b)),
        }
    }
    out.push('"');
    out
}

/// Parse a buffer of encoder-stream bytes into a Vec of instructions for rendering.
pub(in crate::headers::qpack) fn parse_encoder_stream_for_dump(
    bytes: &[u8],
) -> Vec<EncoderInstruction> {
    let mut stream = bytes;
    let mut out = Vec::new();
    while let Ok(Some(instr)) = future::block_on(parse_encoder_instr(usize::MAX, &mut stream)) {
        out.push(instr);
    }
    out
}

/// Parse a header-block record (prefix + field lines). Returns `None` if the prefix fails
/// to parse; otherwise returns the parsed prefix and the field-line instructions.
pub(in crate::headers::qpack) fn parse_header_block_for_dump(
    bytes: &[u8],
) -> Option<(FieldSectionPrefix, Vec<FieldLineInstruction<'_>>)> {
    let (prefix, mut rest) = FieldSectionPrefix::parse(bytes).ok()?;
    let mut lines = Vec::new();
    while !rest.is_empty() {
        match FieldLineInstruction::parse(rest) {
            Ok((instr, remaining)) => {
                lines.push(instr);
                rest = remaining;
            }
            Err(_) => break,
        }
    }
    Some((prefix, lines))
}
