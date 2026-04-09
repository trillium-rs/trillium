use super::DecoderDynamicTable;
use crate::{
    HeaderName, HeaderValue, Headers, Method, Status,
    h3::{H3Error, H3ErrorCode},
    headers::qpack::{
        FieldSection, PseudoHeaders,
        entry_name::QpackEntryName,
        instruction::{
            field_section::{FieldLineInstruction, FieldSectionPrefix},
            validate_value,
        },
        static_table::{PseudoHeaderName, StaticHeaderName, static_entry},
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
    /// dynamic table has received enough entries. Returns an error on protocol violations or
    /// if the encoder stream fails while waiting.
    ///
    /// Duplicate pseudo-headers are silently ignored (first value wins).
    /// Unknown pseudo-headers are rejected per RFC 9114 §4.1.1.
    ///
    /// # Errors
    ///
    /// Returns an error if the encoded bytes cannot be parsed as a valid field section.
    pub(crate) async fn decode(
        &self,
        encoded: &[u8],
        stream_id: u64,
    ) -> Result<FieldSection<'static>, H3Error> {
        let err = || H3ErrorCode::QpackDecompressionFailed;

        let (prefix, mut rest) = FieldSectionPrefix::parse(encoded).map_err(|_| err())?;
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
                .ok_or_else(err)?
        } else {
            required_insert_count
                .checked_add(delta_base)
                .ok_or_else(err)?
        };

        let mut pseudo_headers = PseudoHeaders::default();
        let mut headers = Headers::new();

        while !rest.is_empty() {
            let (instruction, rest_) = FieldLineInstruction::parse(rest).map_err(|_| err())?;
            rest = rest_;

            let field_line =
                apply_instruction(instruction, base, required_insert_count, self).await?;

            match field_line {
                FieldLine::Header(name, value) => {
                    headers.append(name, value);
                }
                FieldLine::Pseudo(pseudo) => pseudo.apply(&mut pseudo_headers),
            }
        }

        if required_insert_count > 0 {
            self.acknowledge_section(stream_id, required_insert_count);
        }

        Ok(FieldSection {
            pseudo_headers,
            headers: Cow::Owned(headers),
        })
    }
}

/// Resolve a parsed [`FieldLineInstruction`] into a [`FieldLine`]. Dynamic-index variants
/// consult `table` (may await entries that aren't yet inserted); literal-value variants
/// carry the value on the instruction itself.
///
/// TODO(n-bit): the `never_indexed` flag on literal instruction variants is currently
/// discarded here. Preserving it for trillium-proxy reverse-proxy correctness requires
/// extending `FieldLine` / `FieldSection` with a per-field never-index flag. See
/// `qpack-n-bit-gap` memory.
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
            Ok(static_table_field_line(*name, value))
        }
        FieldLineInstruction::IndexedDynamic { relative_index } => {
            let abs = base
                .checked_sub(1)
                .and_then(|b| b.checked_sub(relative_index as u64))
                .ok_or_else(err)?;
            let (name, value) = table.get(abs, required_insert_count).await?;
            entry_field_line(name, value).map_err(Into::into)
        }
        FieldLineInstruction::IndexedPostBase { post_base_index } => {
            let abs = base.checked_add(post_base_index as u64).ok_or_else(err)?;
            let (name, value) = table.get(abs, required_insert_count).await?;
            entry_field_line(name, value).map_err(Into::into)
        }
        FieldLineInstruction::LiteralStaticNameRef {
            name_index, value, ..
        } => {
            let (name, _) = static_entry(name_index)?;
            entry_field_line(QpackEntryName::from(*name), value.into_static()).map_err(Into::into)
        }
        FieldLineInstruction::LiteralDynamicNameRef {
            relative_index,
            value,
            ..
        } => {
            let abs = base
                .checked_sub(1)
                .and_then(|b| b.checked_sub(relative_index as u64))
                .ok_or_else(err)?;
            let (name, _) = table.get(abs, required_insert_count).await?;
            entry_field_line(name, value.into_static()).map_err(Into::into)
        }
        FieldLineInstruction::LiteralPostBaseNameRef {
            post_base_index,
            value,
            ..
        } => {
            let abs = base.checked_add(post_base_index as u64).ok_or_else(err)?;
            let (name, _) = table.get(abs, required_insert_count).await?;
            entry_field_line(name, value.into_static()).map_err(Into::into)
        }
        FieldLineInstruction::LiteralLiteralName { name, value, .. } => {
            entry_field_line(name.into_owned(), value.into_static()).map_err(Into::into)
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

/// Build a `FieldLine` from a resolved entry name and an owned value. Used for every path
/// that produces a `FieldLine` from decoded wire bytes or a dynamic-table lookup.
///
/// Rejects values containing CR, LF, or NUL per RFC 9113 §8.2.1 / RFC 9114 §4.2. Values
/// reached via a dynamic-table lookup are pre-validated on insert, so this check is the
/// load-bearing one for literal field-line variants whose values were not seen by the
/// encoder-stream parser.
fn entry_field_line(
    name: QpackEntryName<'_>,
    value: Cow<'static, [u8]>,
) -> Result<FieldLine, H3ErrorCode> {
    let err = || H3ErrorCode::QpackDecompressionFailed;
    validate_value(&value).map_err(|()| err())?;
    match name {
        QpackEntryName::Known(k) => Ok(FieldLine::Header(k.into(), HeaderValue::from(value))),
        QpackEntryName::Unknown(u) => Ok(FieldLine::Header(
            u.into_owned().into(),
            HeaderValue::from(value),
        )),
        QpackEntryName::Pseudo(PseudoHeaderName::Method) => Ok(FieldLine::Pseudo(
            PseudoHeader::Method(Method::parse(&value).map_err(|_| err())?),
        )),
        QpackEntryName::Pseudo(PseudoHeaderName::Status) => {
            Ok(FieldLine::Pseudo(PseudoHeader::Status(
                std::str::from_utf8(&value)
                    .map_err(|_| err())?
                    .parse()
                    .map_err(|_| err())?,
            )))
        }
        QpackEntryName::Pseudo(other) => Ok(FieldLine::Pseudo(PseudoHeader::Other(
            other,
            Some(match value {
                Cow::Borrowed(b) => Cow::Borrowed(std::str::from_utf8(b).map_err(|_| err())?),
                Cow::Owned(b) => Cow::Owned(String::from_utf8(b).map_err(|_| err())?),
            }),
        ))),
    }
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
    // intention: these will eventually be split out into appropriately parsed and validated types
    Other(PseudoHeaderName, Option<Cow<'static, str>>),
}
impl PseudoHeader {
    /// Set the corresponding field on `PseudoHeaders`. First value wins.
    pub(in crate::headers) fn apply(self, pseudos: &mut PseudoHeaders<'static>) {
        match self {
            PseudoHeader::Method(m) => {
                pseudos.method.get_or_insert(m);
            }
            PseudoHeader::Status(s) => {
                pseudos.status.get_or_insert(s);
            }
            PseudoHeader::Other(PseudoHeaderName::Authority, Some(v)) => {
                pseudos.authority.get_or_insert(v);
            }
            PseudoHeader::Other(PseudoHeaderName::Path, Some(v)) => {
                pseudos.path.get_or_insert(v);
            }
            PseudoHeader::Other(PseudoHeaderName::Scheme, Some(v)) => {
                pseudos.scheme.get_or_insert(v);
            }
            PseudoHeader::Other(PseudoHeaderName::Protocol, Some(v)) => {
                pseudos.protocol.get_or_insert(v);
            }
            // Method and Status with the Other variant shouldn't be constructed,
            // but handle gracefully
            PseudoHeader::Other(PseudoHeaderName::Method | PseudoHeaderName::Status, _)
            | PseudoHeader::Other(_, None) => {}
        }
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
