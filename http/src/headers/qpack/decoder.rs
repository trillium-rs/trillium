use super::{
    DecoderDynamicTable, FieldSection, PseudoHeaders, huffman,
    static_table::{PseudoHeaderName, StaticHeaderName, static_entry},
    varint,
};
use crate::{
    HeaderName, HeaderValue, Headers, Method, Status,
    h3::{H3Error, H3ErrorCode},
    headers::qpack::{
        BASE_DELTA_SIGN, INDEXED_FIELD_LINE, INDEXED_STATIC_FLAG, LITERAL_WITH_LITERAL_NAME,
        LITERAL_WITH_NAME_REF, NAME_REF_STATIC_FLAG, POST_BASE_INDEXED, huffman::HuffmanError,
        varint::VarIntError,
    },
};
use core::convert::TryFrom;
use std::{
    borrow::Cow,
    fmt::{self, Display, Formatter},
};

/// Errors that can occur during QPACK decoding.
#[derive(Debug, thiserror::Error, Clone, Copy)]
pub enum DecoderError {
    #[error(transparent)]
    Huffman(#[from] HuffmanError),

    #[error(transparent)]
    VarInt(#[from] VarIntError),

    #[error("static table index {0} out of range (0-98)")]
    InvalidStaticIndex(usize),

    #[error("dynamic table references are not yet supported")]
    DynamicTableUnsupported,

    #[error("unexpected end of field section")]
    UnexpectedEnd,

    #[error("required insert count must be zero without dynamic table")]
    NonZeroInsertCount,

    #[error("invalid header name")]
    InvalidHeaderName,

    #[error("invalid header value")]
    InvalidHeaderValue,

    #[error("method not recongized")]
    UnrecognizedMethod,

    #[error("invalid status")]
    InvalidStatus,
}

/// A decoded field line from a QPACK-encoded header block.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FieldLine {
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
pub(crate) enum PseudoHeader {
    Method(Method),
    Status(Status),
    // intention: these will eventually be split out into appropriately parsed and validated types
    Other(PseudoHeaderName, Option<Cow<'static, str>>),
}
impl PseudoHeader {
    /// Set the corresponding field on `PseudoHeaders`. First value wins.
    fn apply(self, pseudos: &mut PseudoHeaders<'static>) {
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

/// Decode a string literal per RFC 9204 §4.1.2.
///
/// The first byte contains the Huffman flag (H) in its high bit and
/// the string length in the remaining bits with the given prefix size.
///
/// Returns the decoded string bytes and the unconsumed input.
fn decode_string(input: &[u8], prefix_size: u8) -> Result<(Vec<u8>, &[u8]), DecoderError> {
    let [first, ..] = input else {
        return Err(DecoderError::UnexpectedEnd);
    };

    let huffman_encoded = first & (1 << prefix_size) != 0;
    let (length, rest) = varint::decode(input, prefix_size)?;

    if rest.len() < length {
        return Err(DecoderError::UnexpectedEnd);
    }

    let (string_bytes, rest) = rest.split_at(length);

    let decoded = if huffman_encoded {
        huffman::decode(string_bytes)?
    } else {
        string_bytes.to_vec()
    };

    Ok((decoded, rest))
}

impl FieldSection<'static> {
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
    pub(crate) async fn decode_with_dynamic_table(
        encoded: &[u8],
        table: &DecoderDynamicTable,
        stream_id: u64,
    ) -> Result<Self, H3Error> {
        let err = || H3ErrorCode::QpackDecompressionFailed;

        let (encoded_ric, rest) = varint::decode(encoded, 8).map_err(|_| err())?;
        let required_insert_count = table.decode_required_insert_count(encoded_ric)?;
        log::trace!(
            "QPACK decode stream {stream_id}: encoded_ric={encoded_ric} \
             required_insert_count={required_insert_count}"
        );
        let _blocked_guard = table.try_reserve_blocked_stream(required_insert_count)?;

        // The S bit is bit 7 of the first byte of the delta-base field.
        let [delta_base_byte, ..] = rest else {
            return Err(err().into());
        };
        let s_bit = delta_base_byte & BASE_DELTA_SIGN != 0;
        let (delta_base, mut rest) = varint::decode(rest, 7).map_err(|_| err())?;

        let base: u64 = if s_bit {
            required_insert_count
                .checked_sub(delta_base as u64)
                .and_then(|v| v.checked_sub(1))
                .ok_or_else(err)?
        } else {
            required_insert_count
                .checked_add(delta_base as u64)
                .ok_or_else(err)?
        };

        let mut pseudo_headers = PseudoHeaders::default();
        let mut headers = Headers::new();

        while !rest.is_empty() {
            let first = rest[0];

            let field_line: FieldLine = if first & INDEXED_FIELD_LINE != 0 {
                // §4.5.2: Indexed Field Line
                let is_static = first & INDEXED_STATIC_FLAG != 0;
                if is_static {
                    let (fl, rest_) = decode_indexed(rest).map_err(|_| err())?;
                    rest = rest_;
                    fl
                } else {
                    let (relative_index, rest_) = varint::decode(rest, 6).map_err(|_| err())?;
                    rest = rest_;
                    let abs = base
                        .checked_sub(1)
                        .and_then(|b| b.checked_sub(relative_index as u64))
                        .ok_or_else(err)?;
                    let (name, value) = table.get(abs, required_insert_count).await?;
                    FieldLine::Header(name, value)
                }
            } else if first & LITERAL_WITH_NAME_REF != 0 {
                // §4.5.4: Literal Field Line with Name Reference
                let is_static = first & NAME_REF_STATIC_FLAG != 0;
                if is_static {
                    let (fl, rest_) = decode_literal_with_name_ref(rest).map_err(|_| err())?;
                    rest = rest_;
                    fl
                } else {
                    // Dynamic: 4-bit prefix (after N bit at bit 5 and T bit at bit 4)
                    let (relative_index, rest_) = varint::decode(rest, 4).map_err(|_| err())?;
                    let abs = base
                        .checked_sub(1)
                        .and_then(|b| b.checked_sub(relative_index as u64))
                        .ok_or_else(err)?;
                    let (name, _) = table.get(abs, required_insert_count).await?;
                    let (value_bytes, rest_) = decode_string(rest_, 7).map_err(|_| err())?;
                    rest = rest_;
                    FieldLine::Header(name, HeaderValue::parse(&value_bytes))
                }
            } else if first & LITERAL_WITH_LITERAL_NAME != 0 {
                // §4.5.6: Literal Field Line with Literal Name
                let (fl, rest_) = decode_literal_with_literal_name(rest).map_err(|_| err())?;
                rest = rest_;
                fl
            } else if first & POST_BASE_INDEXED != 0 {
                // §4.5.3: Indexed Field Line with Post-Base Index (0001xxxx)
                let (post_base_index, rest_) = varint::decode(rest, 4).map_err(|_| err())?;
                rest = rest_;
                let abs = base.checked_add(post_base_index as u64).ok_or_else(err)?;
                let (name, value) = table.get(abs, required_insert_count).await?;
                FieldLine::Header(name, value)
            } else {
                // §4.5.5: Literal Field Line with Post-Base Name Reference (0000Nxxx)
                let (post_base_index, rest_) = varint::decode(rest, 3).map_err(|_| err())?;
                let abs = base.checked_add(post_base_index as u64).ok_or_else(err)?;
                let (name, _) = table.get(abs, required_insert_count).await?;
                let (value_bytes, rest_) = decode_string(rest_, 7).map_err(|_| err())?;
                rest = rest_;
                FieldLine::Header(name, HeaderValue::parse(&value_bytes))
            };

            match field_line {
                FieldLine::Header(name, value) => {
                    headers.append(name, value);
                }
                FieldLine::Pseudo(pseudo) => pseudo.apply(&mut pseudo_headers),
            }
        }

        if required_insert_count > 0 {
            table.acknowledge_section(stream_id, required_insert_count);
        }

        Ok(Self {
            pseudo_headers,
            headers: Cow::Owned(headers),
        })
    }

    /// Decode a static-only QPACK field section (no dynamic table).
    ///
    /// Returns an error if the field section references the dynamic table.
    ///
    /// # Errors
    ///
    /// Returns an error if the bytes cannot be parsed as a valid field section or if the
    /// field section references the dynamic table.
    pub fn decode(encoded: &[u8]) -> Result<Self, DecoderError> {
        let (required_insert_count, rest) = varint::decode(encoded, 8)?;
        if required_insert_count != 0 {
            return Err(DecoderError::NonZeroInsertCount);
        }
        let (_delta_base, mut rest) = varint::decode(rest, 7)?;

        let mut pseudo_headers = PseudoHeaders::default();
        let mut headers = Headers::new();

        while !rest.is_empty() {
            let first = rest[0];

            let (field_line, rest_) = if first & INDEXED_FIELD_LINE != 0 {
                decode_indexed(rest)?
            } else if first & LITERAL_WITH_NAME_REF != 0 {
                decode_literal_with_name_ref(rest)?
            } else if first & LITERAL_WITH_LITERAL_NAME != 0 {
                decode_literal_with_literal_name(rest)?
            } else {
                return Err(DecoderError::DynamicTableUnsupported);
            };

            rest = rest_;

            match field_line {
                FieldLine::Header(name, value) => {
                    headers.append(name, value);
                }
                FieldLine::Pseudo(pseudo) => pseudo.apply(&mut pseudo_headers),
            }
        }

        Ok(Self {
            pseudo_headers,
            headers: Cow::Owned(headers),
        })
    }
}

/// §4.5.2: Indexed Field Line
fn decode_indexed(input: &[u8]) -> Result<(FieldLine, &[u8]), DecoderError> {
    let first = input[0];
    let is_static = first & LITERAL_WITH_NAME_REF != 0; // T bit

    if !is_static {
        return Err(DecoderError::DynamicTableUnsupported);
    }

    let (index, rest) = varint::decode(input, 6)?;
    let (name, value) = static_entry(index)?;

    let field_line = match name {
        StaticHeaderName::Header(known) => {
            FieldLine::Header(HeaderName::from(*known), HeaderValue::from(*value))
        }
        StaticHeaderName::Pseudo(PseudoHeaderName::Method) => {
            FieldLine::Pseudo(PseudoHeader::Method(Method::try_from(*value).unwrap()))
        }
        StaticHeaderName::Pseudo(PseudoHeaderName::Status) => {
            FieldLine::Pseudo(PseudoHeader::Status(value.parse().unwrap()))
        }
        StaticHeaderName::Pseudo(other) => {
            FieldLine::Pseudo(PseudoHeader::Other(*other, Some(Cow::Borrowed(value))))
        }
    };

    Ok((field_line, rest))
}

/// §4.5.4: Literal Field Line with Name Reference
fn decode_literal_with_name_ref(input: &[u8]) -> Result<(FieldLine, &[u8]), DecoderError> {
    let first = input[0];
    let is_static = first & NAME_REF_STATIC_FLAG != 0; // T bit
    // N bit (first & LITERAL_WITH_LITERAL_NAME) is for intermediary forwarding — we note
    // it but don't act on it yet.

    if !is_static {
        return Err(DecoderError::DynamicTableUnsupported);
    }

    let (index, rest) = varint::decode(input, 4)?;
    let (name, _) = static_entry(index)?;

    let (value_bytes, rest) = decode_string(rest, 7)?;
    let field_line = match name {
        StaticHeaderName::Header(known) => {
            FieldLine::Header(HeaderName::from(*known), HeaderValue::parse(&value_bytes))
        }
        StaticHeaderName::Pseudo(PseudoHeaderName::Method) => {
            FieldLine::Pseudo(PseudoHeader::Method(
                Method::parse(&value_bytes).map_err(|_| DecoderError::UnrecognizedMethod)?,
            ))
        }
        StaticHeaderName::Pseudo(PseudoHeaderName::Status) => {
            FieldLine::Pseudo(PseudoHeader::Status(
                std::str::from_utf8(&value_bytes)
                    .map_err(|_| DecoderError::InvalidStatus)?
                    .parse()
                    .map_err(|_| DecoderError::InvalidStatus)?,
            ))
        }
        StaticHeaderName::Pseudo(pseudo) => FieldLine::Pseudo(PseudoHeader::Other(
            *pseudo,
            Some(Cow::Owned(
                String::from_utf8(value_bytes).map_err(|_| DecoderError::InvalidHeaderValue)?,
            )),
        )),
    };

    Ok((field_line, rest))
}

/// §4.5.6: Literal Field Line with Literal Name
fn decode_literal_with_literal_name(input: &[u8]) -> Result<(FieldLine, &[u8]), DecoderError> {
    // N bit is first & NAME_REF_STATIC_FLAG — noted but not acted on.
    // H bit for name is first & 0x08.
    let (name_bytes, rest) = decode_string(input, 3)?;
    let (value_bytes, rest) = decode_string(rest, 7)?;

    let name = {
        let bytes: &[u8] = &name_bytes;
        HeaderName::parse(bytes)
            .map(|n| n.to_owned())
            .map_err(|_| DecoderError::InvalidHeaderName)
    }?;

    let value = {
        let bytes: &[u8] = &value_bytes;
        HeaderValue::parse(bytes)
    };

    Ok((FieldLine::Header(name, value), rest))
}
