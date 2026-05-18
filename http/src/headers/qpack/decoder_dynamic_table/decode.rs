use super::DecoderDynamicTable;
use crate::{
    HeaderName, HeaderValue, Headers, Method, Status,
    h3::{H3Error, H3ErrorCode},
    headers::{
        entry_name::{EntryName, PseudoHeaderName},
        qpack::{
            FieldSection, PseudoHeaders,
            instruction::{
                field_section::{FieldLineInstruction, FieldSectionPrefix},
                validate_value,
            },
            static_table::{StaticHeaderName, static_entry},
        },
    },
};
use std::{
    borrow::Cow,
    fmt::{self, Display, Formatter},
};

impl DecoderDynamicTable {
    /// Decode a QPACK-encoded field section, consulting the dynamic table as needed.
    ///
    /// If the field section's Required Insert Count is greater than zero, waits until the
    /// dynamic table has received enough entries. Returns an error on protocol violations
    /// or if the encoder stream fails while waiting.
    ///
    /// Duplicate pseudo-headers, pseudo-headers after a regular header, and unknown
    /// pseudo-headers are rejected as malformed messages (stream error
    /// `H3_MESSAGE_ERROR`).
    ///
    /// # Errors
    ///
    /// Returns an error if the encoded bytes cannot be parsed as a valid field section.
    pub(crate) async fn decode(
        &self,
        encoded: &[u8],
        stream_id: u64,
    ) -> Result<FieldSection<'static>, H3Error> {
        let codec_err = || H3ErrorCode::QpackDecompressionFailed;

        // FieldSectionPrefix parsing only fails on integer-prefix codec errors, never on
        // the InvalidHeaderName branch — collapsing CompressionError variants to
        // QpackDecompressionFailed is correct here.
        let (prefix, mut rest) = FieldSectionPrefix::parse(encoded).map_err(|_| codec_err())?;
        let required_insert_count =
            self.decode_required_insert_count(prefix.encoded_required_insert_count)?;
        log::trace!(
            "QPACK decode stream {stream_id}: encoded_ric={} \
             required_insert_count={required_insert_count}",
            prefix.encoded_required_insert_count,
        );
        let _blocked_guard = self.try_reserve_blocked_stream(required_insert_count)?;

        let delta_base = prefix.delta_base as u64;
        let base: u64 = if prefix.base_is_negative {
            required_insert_count
                .checked_sub(delta_base)
                .and_then(|v| v.checked_sub(1))
                .ok_or_else(codec_err)?
        } else {
            required_insert_count
                .checked_add(delta_base)
                .ok_or_else(codec_err)?
        };

        let mut pseudo_headers = PseudoHeaders::default();
        let mut headers = Headers::new();
        let mut saw_regular = false;
        // Once any malformed-message condition fires, the section is malformed. We continue
        // parsing the remaining instructions instead of bailing immediately so the decoder
        // dynamic table stays in sync with the encoder (every dynamic reference in the
        // section must be applied), then surface the error at the end of the section.
        let mut malformed = false;

        while !rest.is_empty() {
            // Use `?` so `CompressionError::InvalidHeaderName` (e.g. unknown pseudo,
            // uppercase chars) routes through `From<CompressionError> for H3Error` to
            // MessageError; codec failures still surface as QpackDecompressionFailed.
            // Aborting mid-section here is safe: HEADERS instructions only *read* from the
            // dynamic table — they don't mutate it — so table state stays consistent with
            // the encoder regardless of how many later instructions we skip.
            let (instruction, rest_) = FieldLineInstruction::parse(rest)?;
            rest = rest_;

            let field_line =
                apply_instruction(instruction, base, required_insert_count, self).await?;

            match field_line {
                FieldLine::Header(name, value) => {
                    headers.append(name, value);
                    saw_regular = true;
                }
                FieldLine::Pseudo(pseudo) => {
                    if saw_regular {
                        log::trace!("QPACK decode: pseudo-header after regular: {pseudo:?}");
                        malformed = true;
                    } else if !pseudo.try_apply(&mut pseudo_headers) {
                        log::trace!("QPACK decode: duplicate pseudo-header");
                        malformed = true;
                    }
                }
            }
        }

        if malformed {
            return Err(H3ErrorCode::MessageError.into());
        }

        if required_insert_count > 0 {
            self.acknowledge_section(stream_id, required_insert_count);
        }

        Ok(FieldSection::from_owned(pseudo_headers, headers))
    }
}

/// Resolve a parsed [`FieldLineInstruction`] into a [`FieldLine`]. Dynamic-index variants
/// consult `table` (may await entries that aren't yet inserted); literal-value variants
/// carry the value on the instruction itself. The N (Never-Indexed) bit on the four
/// literal variants is lifted onto the produced [`HeaderValue`] so downstream encoders can
/// re-emit it faithfully.
async fn apply_instruction(
    instruction: FieldLineInstruction<'_>,
    base: u64,
    required_insert_count: u64,
    table: &DecoderDynamicTable,
) -> Result<FieldLine, H3Error> {
    let err = || H3ErrorCode::QpackDecompressionFailed;
    match instruction {
        FieldLineInstruction::IndexedStatic { index } => {
            let (name, value) = static_entry(index)?;
            log::trace!("IndexedStatic {name}: {value:?}");
            Ok(static_table_field_line(*name, value))
        }
        FieldLineInstruction::IndexedDynamic { relative_index } => {
            let abs = base
                .checked_sub(1)
                .and_then(|b| b.checked_sub(relative_index as u64))
                .ok_or_else(err)?;
            let (name, value) = table.get(abs, required_insert_count).await?;
            log::trace!("IndexedDynamic {name}: {}", String::from_utf8_lossy(&value));
            entry_field_line(name, value, false).map_err(Into::into)
        }
        FieldLineInstruction::IndexedPostBase { post_base_index } => {
            let abs = base.checked_add(post_base_index as u64).ok_or_else(err)?;
            let (name, value) = table.get(abs, required_insert_count).await?;
            log::trace!(
                "IndexedPostBase {name}: {}",
                String::from_utf8_lossy(&value)
            );
            entry_field_line(name, value, false).map_err(Into::into)
        }
        FieldLineInstruction::LiteralStaticNameRef {
            name_index,
            value,
            never_indexed,
        } => {
            let (name, _) = static_entry(name_index)?;
            log::trace!("LiteralStaticNameRef {name}: {value:?}");
            entry_field_line(EntryName::from(*name), value.into_static(), never_indexed)
                .map_err(Into::into)
        }
        FieldLineInstruction::LiteralDynamicNameRef {
            relative_index,
            value,
            never_indexed,
        } => {
            let abs = base
                .checked_sub(1)
                .and_then(|b| b.checked_sub(relative_index as u64))
                .ok_or_else(err)?;
            let (name, _) = table.get(abs, required_insert_count).await?;
            log::trace!("LiteralDynamicNameRef {name}: {value:?}");
            entry_field_line(name, value.into_static(), never_indexed).map_err(Into::into)
        }
        FieldLineInstruction::LiteralPostBaseNameRef {
            post_base_index,
            value,
            never_indexed,
        } => {
            let abs = base.checked_add(post_base_index as u64).ok_or_else(err)?;
            let (name, _) = table.get(abs, required_insert_count).await?;
            log::trace!("LiteralPostBaseNameRef {name}: {value:?}");
            entry_field_line(name, value.into_static(), never_indexed).map_err(Into::into)
        }
        FieldLineInstruction::LiteralLiteralName {
            name,
            value,
            never_indexed,
        } => {
            log::trace!("LiteralLiteralName {name}: {value:?}");
            entry_field_line(name.into_owned(), value.into_static(), never_indexed)
                .map_err(Into::into)
        }
    }
}

/// Build a `FieldLine` from a static-table entry (borrowed static name + value).
/// Infallible — static-table method/status values are pre-validated by construction.
fn static_table_field_line(name: StaticHeaderName, value: &'static str) -> FieldLine {
    match name {
        StaticHeaderName::Header(known) => {
            FieldLine::Header(HeaderName::from(known), HeaderValue::from(value))
        }
        StaticHeaderName::Pseudo(PseudoHeaderName::Method) => {
            FieldLine::Pseudo(PseudoHeader::Method(Method::try_from(value).unwrap()))
        }
        StaticHeaderName::Pseudo(PseudoHeaderName::Status) => {
            FieldLine::Pseudo(PseudoHeader::Status(value.parse().unwrap()))
        }
        StaticHeaderName::Pseudo(other) => {
            FieldLine::Pseudo(PseudoHeader::Other(other, Some(Cow::Borrowed(value))))
        }
    }
}

/// Build a `FieldLine` from a resolved entry name and an owned value.
///
/// Rejects values containing CR, LF, or NUL. Values reached via a dynamic-table lookup are
/// pre-validated on insert, so this check is the load-bearing one for literal field-line
/// variants whose values were not seen by the encoder-stream parser.
fn entry_field_line(
    name: EntryName<'_>,
    value: Cow<'static, [u8]>,
    never_indexed: bool,
) -> Result<FieldLine, H3ErrorCode> {
    let err = || H3ErrorCode::QpackDecompressionFailed;
    validate_value(&value).map_err(|_| err())?;
    let header_name = match name {
        EntryName::Known(k) => HeaderName::from(k),
        EntryName::UnknownStatic(s) => HeaderName::from(s),
        EntryName::Unknown(u) => u.into_owned().into(),
        EntryName::Pseudo(PseudoHeaderName::Method) => {
            // The N bit on a pseudo header has no place to live — pseudos route to typed
            // fields on Conn (Method/Status/Cow), not into Headers — so we drop it. For
            // proxy round-trip this is fine: pseudos are rebuilt from the typed fields,
            // not re-emitted from the original wire bits.
            return Ok(FieldLine::Pseudo(PseudoHeader::Method(
                Method::parse(&value).map_err(|_| err())?,
            )));
        }
        EntryName::Pseudo(PseudoHeaderName::Status) => {
            return Ok(FieldLine::Pseudo(PseudoHeader::Status(
                std::str::from_utf8(&value)
                    .map_err(|_| err())?
                    .parse()
                    .map_err(|_| err())?,
            )));
        }
        EntryName::Pseudo(other) => {
            return Ok(FieldLine::Pseudo(PseudoHeader::Other(
                other,
                Some(match value {
                    Cow::Borrowed(b) => Cow::Borrowed(std::str::from_utf8(b).map_err(|_| err())?),
                    Cow::Owned(b) => Cow::Owned(String::from_utf8(b).map_err(|_| err())?),
                }),
            )));
        }
    };
    let mut header_value = HeaderValue::from(value);
    header_value.set_never_indexed(never_indexed);
    Ok(FieldLine::Header(header_name, header_value))
}

/// A decoded field line from a QPACK-encoded header block.
#[derive(Debug, PartialEq, Eq)]
enum FieldLine {
    /// A regular HTTP header.
    Header(HeaderName<'static>, HeaderValue),
    /// A pseudo-header (`:method`, `:path`, etc.) which the caller
    /// should route to Conn fields rather than the Headers map.
    Pseudo(PseudoHeader),
}
impl Display for FieldLine {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            FieldLine::Header(header_name, header_value) => {
                write!(f, "{header_name}: {header_value}")
            }
            FieldLine::Pseudo(pseudo_header) => write!(f, "{pseudo_header}"),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum PseudoHeader {
    Method(Method),
    Status(Status),
    Other(PseudoHeaderName, Option<Cow<'static, str>>),
}
impl PseudoHeader {
    /// Set the corresponding field on `PseudoHeaders`. Returns `false` if the slot was
    /// already occupied (duplicate pseudo-header) — the caller MUST treat this as a
    /// malformed message.
    ///
    /// Returns `true` for the no-op `Method`/`Status` constructed via the `Other` variant
    /// (which shouldn't happen but is handled gracefully) and for the value-less `Other`
    /// variant — neither writes into a slot, so neither can collide.
    pub(in crate::headers) fn try_apply(self, pseudos: &mut PseudoHeaders<'static>) -> bool {
        match self {
            PseudoHeader::Method(m) => {
                if pseudos.method().is_some() {
                    return false;
                }
                *pseudos.method_mut() = Some(m);
            }
            PseudoHeader::Status(s) => {
                if pseudos.status().is_some() {
                    return false;
                }
                *pseudos.status_mut() = Some(s);
            }
            PseudoHeader::Other(PseudoHeaderName::Authority, Some(v)) => {
                if pseudos.authority().is_some() {
                    return false;
                }
                *pseudos.authority_mut() = Some(v);
            }
            PseudoHeader::Other(PseudoHeaderName::Path, Some(v)) => {
                if pseudos.path().is_some() {
                    return false;
                }
                *pseudos.path_mut() = Some(v);
            }
            PseudoHeader::Other(PseudoHeaderName::Scheme, Some(v)) => {
                if pseudos.scheme().is_some() {
                    return false;
                }
                *pseudos.scheme_mut() = Some(v);
            }
            PseudoHeader::Other(PseudoHeaderName::Protocol, Some(v)) => {
                if pseudos.protocol().is_some() {
                    return false;
                }
                *pseudos.protocol_mut() = Some(v);
            }
            // Method and Status with the Other variant shouldn't be constructed,
            // but handle gracefully — no slot written, no collision possible.
            PseudoHeader::Other(PseudoHeaderName::Method | PseudoHeaderName::Status, _)
            | PseudoHeader::Other(_, None) => {}
        }
        true
    }
}

impl Display for PseudoHeader {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            PseudoHeader::Method(method) => write!(f, ":method: {method}"),
            PseudoHeader::Status(status) => write!(f, ":status: {status}"),
            PseudoHeader::Other(pseudo_header_name, Some(value)) => {
                write!(f, "{pseudo_header_name}: {value}")
            }
            PseudoHeader::Other(pseudo_header_name, None) => write!(f, "{pseudo_header_name}:"),
        }
    }
}
