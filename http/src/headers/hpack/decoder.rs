//! HPACK decoder (RFC 7541 §6).
//!
//! Consumes a complete header block (HEADERS + CONTINUATION already reassembled by the
//! caller) and produces a [`FieldSection`]. Stateful across calls: the [`DynamicTable`]
//! passed in persists between header blocks on a single HTTP/2 connection.
//!
//! Handles all five wire representations:
//! - §6.1 Indexed Header Field (`1xxxxxxx`)
//! - §6.2.1 Literal Header Field with Incremental Indexing (`01xxxxxx`)
//! - §6.2.2 Literal Header Field without Indexing (`0000xxxx`)
//! - §6.2.3 Literal Header Field Never Indexed (`0001xxxx`)
//! - §6.3 Dynamic Table Size Update (`001xxxxx`)
//!
//! §4.2 ordering is enforced: size updates MUST appear before any field representation.
//! `protocol_max_table_size` (the peer's `SETTINGS_HEADER_TABLE_SIZE`) is an upper bound on
//! size updates; exceeding it is a decoding error (§6.3).
//!
//! The "Never Indexed" bit from §6.2.3 is preserved on decode but not yet plumbed through
#[cfg(test)]
mod rfc7541_vectors;
#[cfg(test)]
mod tests;

use super::{
    HpackDecodeError,
    dynamic_table::{DynamicTable, Entry},
    static_table::{StaticHeaderName, static_entry},
};
use crate::{
    HeaderName, HeaderValue, Headers, Method, Status,
    headers::{
        compression_error::CompressionError,
        entry_name::{EntryName, PseudoHeaderName},
        field_section::{FieldLineValue, FieldSection, PseudoHeaders},
        huffman, integer_prefix,
    },
};
use std::borrow::Cow;

/// Spec-defined request malformations (RFC 9113 §8.1.2) the decoder is in a position to
/// detect. Each variant maps to a stream-level `PROTOCOL_ERROR`.
///
/// Intentionally payload-free — the caller only needs to know *that* something was
/// malformed to decide the response; which specific pseudo triggered it lives in the
/// debug log via trace-level logging inside the decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MalformedRequest {
    /// A pseudo-header field appeared twice in the same block (§8.1.2.3). First occurrence
    /// is kept in the returned section; the duplicate is dropped. This reason still
    /// surfaces as an error so the caller doesn't proceed with a silently truncated set.
    DuplicatePseudoHeader,
    /// A pseudo-header appeared after a regular header (§8.1.2.1). Pseudos MUST precede
    /// every regular header in the block.
    PseudoHeaderAfterRegular,
}

// First-byte masks (RFC 7541 §6).
//
// Patterns listed in the order [`decode`] tests them in — each mask identifies a family of
// representations; the next-more-specific mask refines within it.
const INDEXED: u8 = 0b1000_0000; // §6.1    1xxxxxxx
const LITERAL_WITH_INDEXING: u8 = 0b0100_0000; // §6.2.1  01xxxxxx
const SIZE_UPDATE: u8 = 0b0010_0000; // §6.3    001xxxxx
const LITERAL_NEVER_INDEXED: u8 = 0b0001_0000; // §6.2.3  0001xxxx
// §6.2.2 Literal Header Field without Indexing: 0000xxxx — the "everything else" case.

/// Decode an HPACK-encoded header block into a [`FieldSection`].
///
/// `protocol_max_table_size` is the maximum value a §6.3 Dynamic Table Size Update is
/// allowed to carry — typically the peer's `SETTINGS_HEADER_TABLE_SIZE`.
///
/// # Errors
///
/// Returns [`CompressionError`] on any RFC 7541 protocol violation: invalid static index,
/// index 0 in an Indexed representation, oversized size update, malformed integer/string,
/// misordered size update, Huffman decode failure, or invalid header name.
pub(in crate::headers) fn decode(
    mut input: &[u8],
    table: &mut DynamicTable,
    protocol_max_table_size: usize,
) -> Result<FieldSection<'static>, HpackDecodeError> {
    let mut pseudo_headers = PseudoHeaders::default();
    let mut headers = Headers::new();
    let mut size_updates_allowed = true;
    // §8.1.2.1 ordering: pseudos MUST precede all regular headers. Set once the first
    // regular header lands; any subsequent pseudo is malformed.
    let mut saw_regular = false;
    // First malformation detected. We keep decoding after detection so the dynamic table
    // stays consistent with the peer's model (aborting mid-block would leave their
    // incremental-indexing inserts applied on their side but not ours).
    let mut malformed: Option<MalformedRequest> = None;

    while !input.is_empty() {
        let first = input[0];

        if first & INDEXED != 0 {
            // §6.1 Indexed Header Field
            let (index, rest) = integer_prefix::decode(input, 7)?;
            if index == 0 {
                return Err(CompressionError::InvalidStaticIndex(0).into());
            }
            size_updates_allowed = false;
            input = rest;
            emit_indexed(
                index,
                table,
                &mut pseudo_headers,
                &mut headers,
                &mut saw_regular,
                &mut malformed,
            )?;
        } else if first & LITERAL_WITH_INDEXING != 0 {
            // §6.2.1 Literal Header Field with Incremental Indexing
            let (index, rest) = integer_prefix::decode(input, 6)?;
            let (name, value, rest) = read_literal_name_value(rest, index, table)?;
            size_updates_allowed = false;
            input = rest;

            // Insert into the dynamic table — §6.2.1: "A literal header field with
            // incremental indexing representation results in appending a header field to
            // the decoded header list and inserting it as a new entry into the dynamic
            // table."
            table.insert(name.clone(), value.clone());
            emit_literal(
                name,
                value,
                &mut pseudo_headers,
                &mut headers,
                &mut saw_regular,
                &mut malformed,
            )?;
        } else if first & SIZE_UPDATE != 0 {
            // §6.3 Dynamic Table Size Update
            if !size_updates_allowed {
                // §4.2: updates MUST occur before any field representation.
                return Err(CompressionError::UnexpectedEnd.into());
            }
            let (new_max, rest) = integer_prefix::decode(input, 5)?;
            if new_max > protocol_max_table_size {
                // §6.3: MUST treat as decoding error.
                return Err(CompressionError::InvalidStaticIndex(new_max).into());
            }
            input = rest;
            table.set_max_size(new_max);
        } else if first & LITERAL_NEVER_INDEXED != 0 {
            // §6.2.3 Literal Header Field Never Indexed
            let (index, rest) = integer_prefix::decode(input, 4)?;
            let (name, value, rest) = read_literal_name_value(rest, index, table)?;
            size_updates_allowed = false;
            input = rest;
            // TODO(n-bit): preserve the "never indexed" signal through to intermediaries.
            emit_literal(
                name,
                value,
                &mut pseudo_headers,
                &mut headers,
                &mut saw_regular,
                &mut malformed,
            )?;
        } else {
            // §6.2.2 Literal Header Field without Indexing — 0000xxxx prefix
            let (index, rest) = integer_prefix::decode(input, 4)?;
            let (name, value, rest) = read_literal_name_value(rest, index, table)?;
            size_updates_allowed = false;
            input = rest;
            emit_literal(
                name,
                value,
                &mut pseudo_headers,
                &mut headers,
                &mut saw_regular,
                &mut malformed,
            )?;
        }
    }

    if let Some(reason) = malformed {
        return Err(HpackDecodeError::MalformedRequest(reason));
    }
    Ok(FieldSection::from_owned(pseudo_headers, headers))
}

/// Emit an `Indexed` (§6.1) field line by resolving `index` against the static or dynamic
/// table and routing to pseudo/regular header accumulators.
fn emit_indexed(
    index: usize,
    table: &DynamicTable,
    pseudo_headers: &mut PseudoHeaders<'static>,
    headers: &mut Headers,
    saw_regular: &mut bool,
    malformed: &mut Option<MalformedRequest>,
) -> Result<(), CompressionError> {
    if index <= 61 {
        let (name, value) = static_entry(index)?;
        let value_bytes = FieldLineValue::Static(value.as_bytes());
        emit_from_entry_ref(
            *name,
            value_bytes,
            pseudo_headers,
            headers,
            saw_regular,
            malformed,
        )
    } else {
        let dyn_index = index - 61;
        let entry = table
            .get(dyn_index)
            .ok_or(CompressionError::InvalidStaticIndex(index))?;
        emit_from_entry(entry, pseudo_headers, headers, saw_regular, malformed)
    }
}

/// Read the `(name, value)` pair of a §6.2.x literal representation.
///
/// `index` is the first-byte prefix integer already consumed from `input`. When `index > 0`
/// the name comes from the static or dynamic table; when `0`, the name is a literal string
/// that follows.
fn read_literal_name_value<'a>(
    input: &'a [u8],
    index: usize,
    table: &DynamicTable,
) -> Result<(EntryName<'static>, FieldLineValue<'static>, &'a [u8]), CompressionError> {
    let (name, rest) = if index == 0 {
        let (bytes, rest) = read_string(input)?;
        let name = EntryName::try_from(bytes).map_err(|_| CompressionError::InvalidHeaderName)?;
        (name, rest)
    } else if index <= 61 {
        let (name, _) = static_entry(index)?;
        (EntryName::from(*name), input)
    } else {
        // Dynamic-table name reference. Clone the name — the caller may mutate the table
        // (§6.2.1 insert) while this EntryName is still live.
        let dyn_index = index - 61;
        let entry = table
            .get(dyn_index)
            .ok_or(CompressionError::InvalidStaticIndex(index))?;
        (entry.name.clone(), input)
    };

    let (value_bytes, rest) = read_string(rest)?;
    let value = FieldLineValue::Owned(value_bytes);
    Ok((name, value, rest))
}

/// Read a §5.2 string literal: H-flag (1 bit) + length (7-bit prefix integer) + bytes.
/// Returns the decoded bytes (Huffman-decoded if H was set) and the unconsumed tail.
fn read_string(input: &[u8]) -> Result<(Vec<u8>, &[u8]), CompressionError> {
    let [first, ..] = input else {
        return Err(CompressionError::UnexpectedEnd);
    };
    let huffman_encoded = first & 0x80 != 0;
    let (length, rest) = integer_prefix::decode(input, 7)?;
    if rest.len() < length {
        return Err(CompressionError::UnexpectedEnd);
    }
    let (bytes, rest) = rest.split_at(length);
    let decoded = if huffman_encoded {
        huffman::decode(bytes)?
    } else {
        bytes.to_vec()
    };
    Ok((decoded, rest))
}

/// Route an owned-form `(EntryName, FieldLineValue)` into pseudos or headers.
fn emit_literal(
    name: EntryName<'static>,
    value: FieldLineValue<'static>,
    pseudo_headers: &mut PseudoHeaders<'static>,
    headers: &mut Headers,
    saw_regular: &mut bool,
    malformed: &mut Option<MalformedRequest>,
) -> Result<(), CompressionError> {
    emit_from_entry(
        &Entry { name, value },
        pseudo_headers,
        headers,
        saw_regular,
        malformed,
    )
}

/// Route a `StaticHeaderName` + static `FieldLineValue` into pseudos or headers.
fn emit_from_entry_ref(
    name: StaticHeaderName,
    value: FieldLineValue<'static>,
    pseudo_headers: &mut PseudoHeaders<'static>,
    headers: &mut Headers,
    saw_regular: &mut bool,
    malformed: &mut Option<MalformedRequest>,
) -> Result<(), CompressionError> {
    let entry_name: EntryName<'static> = name.into();
    emit_from_entry(
        &Entry {
            name: entry_name,
            value,
        },
        pseudo_headers,
        headers,
        saw_regular,
        malformed,
    )
}

/// Shared emission path — takes a borrowed entry (owned by caller, cloned into Headers when
/// needed) and dispatches to pseudo/header accumulators. Tracks §8.1.2.1 ordering
/// (pseudos-before-regulars) and §8.1.2.3 pseudo uniqueness — both surface as
/// [`MalformedRequest`] via the shared `malformed` slot so the caller can translate to the
/// appropriate stream-level error.
fn emit_from_entry(
    entry: &Entry,
    pseudo_headers: &mut PseudoHeaders<'static>,
    headers: &mut Headers,
    saw_regular: &mut bool,
    malformed: &mut Option<MalformedRequest>,
) -> Result<(), CompressionError> {
    let value_bytes: &[u8] = entry.value.as_bytes();
    match &entry.name {
        EntryName::Known(k) => {
            *saw_regular = true;
            headers.append(
                HeaderName::from(*k),
                HeaderValue::from(value_bytes.to_vec()),
            );
        }
        EntryName::Unknown(u) => {
            *saw_regular = true;
            headers.append(
                HeaderName::from(u.clone().into_owned()),
                HeaderValue::from(value_bytes.to_vec()),
            );
        }
        EntryName::UnknownStatic(s) => {
            *saw_regular = true;
            headers.append(
                HeaderName::from(*s),
                HeaderValue::from(value_bytes.to_vec()),
            );
        }
        EntryName::Pseudo(pseudo) => {
            if *saw_regular {
                log::trace!("hpack: pseudo-header after regular: {pseudo:?}");
                malformed.get_or_insert(MalformedRequest::PseudoHeaderAfterRegular);
            }
            insert_pseudo(*pseudo, value_bytes, pseudo_headers, malformed)?;
        }
    }
    Ok(())
}

/// Insert a decoded pseudo-header with first-wins semantics, flagging duplicates via the
/// shared `malformed` slot (§8.1.2.3). The second occurrence is dropped; the caller is
/// expected to fail the stream once decode returns, so preserving faithful "original order"
/// content doesn't matter — the invariant that matters is that `malformed` is set.
fn insert_pseudo(
    pseudo: PseudoHeaderName,
    value_bytes: &[u8],
    pseudo_headers: &mut PseudoHeaders<'static>,
    malformed: &mut Option<MalformedRequest>,
) -> Result<(), CompressionError> {
    let slot_filled = match pseudo {
        PseudoHeaderName::Method => pseudo_headers.method().is_some(),
        PseudoHeaderName::Status => pseudo_headers.status().is_some(),
        PseudoHeaderName::Authority => pseudo_headers.authority().is_some(),
        PseudoHeaderName::Path => pseudo_headers.path().is_some(),
        PseudoHeaderName::Scheme => pseudo_headers.scheme().is_some(),
        PseudoHeaderName::Protocol => pseudo_headers.protocol().is_some(),
    };
    if slot_filled {
        log::trace!("hpack: duplicate pseudo-header: {pseudo:?}");
        malformed.get_or_insert(MalformedRequest::DuplicatePseudoHeader);
        return Ok(());
    }

    match pseudo {
        PseudoHeaderName::Method => {
            let m = Method::parse(value_bytes).map_err(|_| CompressionError::InvalidHeaderName)?;
            pseudo_headers.set_method(Some(m));
        }
        PseudoHeaderName::Status => {
            let s: Status = std::str::from_utf8(value_bytes)
                .map_err(|_| CompressionError::InvalidHeaderName)?
                .parse()
                .map_err(|_| CompressionError::InvalidHeaderName)?;
            pseudo_headers.set_status(Some(s));
        }
        PseudoHeaderName::Authority
        | PseudoHeaderName::Path
        | PseudoHeaderName::Scheme
        | PseudoHeaderName::Protocol => {
            let s = std::str::from_utf8(value_bytes)
                .map_err(|_| CompressionError::InvalidHeaderName)?
                .to_owned();
            let cow: Cow<'static, str> = Cow::Owned(s);
            match pseudo {
                PseudoHeaderName::Authority => pseudo_headers.set_authority(Some(cow)),
                PseudoHeaderName::Path => pseudo_headers.set_path(Some(cow)),
                PseudoHeaderName::Scheme => pseudo_headers.set_scheme(Some(cow)),
                PseudoHeaderName::Protocol => pseudo_headers.set_protocol(Some(cow)),
                PseudoHeaderName::Method | PseudoHeaderName::Status => unreachable!(),
            };
        }
    }
    Ok(())
}
