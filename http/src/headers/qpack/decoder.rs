use super::{
    FieldSection, PseudoHeaders, huffman,
    static_table::{PseudoHeaderName, StaticHeaderName, static_entry},
    varint,
};
use crate::{HeaderName, HeaderValue, Headers, Method, Status};
use core::convert::TryFrom;
use std::{borrow::Cow, fmt::Display};

/// Errors that can occur during QPACK decoding.
#[derive(Debug, thiserror::Error, Clone, Copy)]
pub enum DecoderError {
    #[error(transparent)]
    Huffman(#[from] huffman::HuffmanError),

    #[error(transparent)]
    VarInt(#[from] varint::VarIntError),

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

    #[error("unknown pseudo-header")]
    UnknownPseudoHeader,
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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
    fn apply(self, pseudos: &mut PseudoHeaders<'static>) -> Result<(), DecoderError> {
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
            PseudoHeader::Other(PseudoHeaderName::Method | PseudoHeaderName::Status, _) => {}
            PseudoHeader::Other(_, None) => {}
        }
        Ok(())
    }
}

impl Display for PseudoHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
    /// Decode a QPACK-encoded field section into pseudo-headers and headers.
    ///
    /// This currently supports only static table references. Dynamic
    /// table references will return an error.
    ///
    /// Duplicate pseudo-headers are silently ignored (first value wins).
    /// Unknown pseudo-headers are rejected per RFC 9114 §4.1.1.
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
            let field_line;

            if first & 0x80 != 0 {
                (field_line, rest) = decode_indexed(rest)?;
            } else if first & 0x40 != 0 {
                (field_line, rest) = decode_literal_with_name_ref(rest)?;
            } else if first & 0x20 != 0 {
                (field_line, rest) = decode_literal_with_literal_name(rest)?;
            } else if first & 0x10 != 0 {
                return Err(DecoderError::DynamicTableUnsupported);
            } else {
                return Err(DecoderError::DynamicTableUnsupported);
            }

            match field_line {
                FieldLine::Header(name, value) => {
                    headers.append(name, value);
                }
                FieldLine::Pseudo(pseudo) => pseudo.apply(&mut pseudo_headers)?,
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
    let is_static = first & 0x40 != 0; // T bit

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
    let is_static = first & 0x10 != 0; // T bit
    // N bit (first & 0x20) is for intermediary forwarding — we note
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
    // N bit is first & 0x10 — noted but not acted on.
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
