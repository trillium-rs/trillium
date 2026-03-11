use super::{
    FieldSection, INDEXED_FIELD_LINE, INDEXED_STATIC_FLAG, LITERAL_WITH_LITERAL_NAME,
    LITERAL_WITH_NAME_REF, NAME_REF_STATIC_FLAG, huffman,
    static_table::{StaticLookup, static_table_lookup},
    varint,
};
use crate::{Method, Status};

impl FieldSection<'_> {
    /// Encode a QPACK field section from pseudo-headers and headers.
    ///
    /// This currently uses only the static table (no dynamic table).
    pub fn encode(&self, buf: &mut Vec<u8>) {
        let Self {
            pseudo_headers,
            headers,
        } = self;

        // §4.5.1: Prefix — required insert count = 0, delta base = 0
        buf.extend_from_slice(&varint::encode(0, 8));
        buf.extend_from_slice(&varint::encode(0, 7));

        if let Some(method) = pseudo_headers.method {
            encode_method(method, buf);
        }
        if let Some(status) = pseudo_headers.status {
            encode_status(status, buf);
        }
        if let Some(path) = &pseudo_headers.path {
            encode_pseudo_string(b":path", path.as_ref(), 1, buf);
        }
        if let Some(scheme) = &pseudo_headers.scheme {
            encode_pseudo_string(b":scheme", scheme.as_ref(), 22, buf);
        }
        if let Some(authority) = &pseudo_headers.authority {
            encode_pseudo_string(b":authority", authority.as_ref(), 0, buf);
        }
        if let Some(protocol) = &pseudo_headers.protocol {
            encode_literal_with_literal_name(b":protocol", protocol.as_ref().as_bytes(), buf);
        }

        for (name, values) in &**headers {
            let name_bytes = name.as_ref().as_bytes();
            for value in values {
                let value_bytes: &[u8] = value.as_ref();
                let lookup = static_table_lookup(&name, value);
                encode_by_lookup(lookup, name_bytes, value_bytes, buf);
            }
        }
    }
}

/// §4.5.2: Indexed Field Line (static table)
fn encode_indexed(index: u8, buf: &mut Vec<u8>) {
    let start = buf.len();
    buf.extend_from_slice(&varint::encode(index as usize, 6));
    buf[start] |= INDEXED_FIELD_LINE | INDEXED_STATIC_FLAG;
}

/// §4.5.4: Literal Field Line with Name Reference (static table)
fn encode_literal_with_name_ref(index: u8, value: &[u8], buf: &mut Vec<u8>) {
    let start = buf.len();
    buf.extend_from_slice(&varint::encode(index as usize, 4));
    buf[start] |= LITERAL_WITH_NAME_REF | NAME_REF_STATIC_FLAG;
    encode_string(value, 7, buf);
}

/// §4.5.6: Literal Field Line with Literal Name
fn encode_literal_with_literal_name(name: &[u8], value: &[u8], buf: &mut Vec<u8>) {
    let start = buf.len();
    encode_string(name, 3, buf);
    buf[start] |= LITERAL_WITH_LITERAL_NAME;
    encode_string(value, 7, buf);
}

fn encode_method(method: Method, buf: &mut Vec<u8>) {
    let index = match method {
        Method::Connect => Some(15),
        Method::Delete => Some(16),
        Method::Get => Some(17),
        Method::Head => Some(18),
        Method::Options => Some(19),
        Method::Post => Some(20),
        Method::Put => Some(21),
        _ => None,
    };
    match index {
        Some(i) => encode_indexed(i, buf),
        None => encode_literal_with_name_ref(15, method.as_str().as_bytes(), buf),
    }
}

fn encode_status(status: Status, buf: &mut Vec<u8>) {
    let index = match status {
        Status::EarlyHints => Some(24),
        Status::Ok => Some(25),
        Status::NotModified => Some(26),
        Status::NotFound => Some(27),
        Status::ServiceUnavailable => Some(28),
        Status::Continue => Some(63),
        Status::NoContent => Some(64),
        Status::PartialContent => Some(65),
        Status::Found => Some(66),
        Status::BadRequest => Some(67),
        Status::Forbidden => Some(68),
        Status::MisdirectedRequest => Some(69),
        Status::TooEarly => Some(70),
        Status::InternalServerError => Some(71),
        _ => None,
    };
    match index {
        Some(i) => encode_indexed(i, buf),
        None => encode_literal_with_name_ref(24, status.code().as_bytes(), buf),
    }
}

/// Encode a pseudo-header with a string value (:path, :scheme, :authority).
/// `name_match_index` is the static table index for name-only lookup.
fn encode_pseudo_string(name: &[u8], value: &str, name_match_index: u8, buf: &mut Vec<u8>) {
    let full_match = match (name, value) {
        (b":path", "/") => Some(1),
        (b":scheme", "http") => Some(22),
        (b":scheme", "https") => Some(23),
        _ => None,
    };
    match full_match {
        Some(i) => encode_indexed(i, buf),
        None => encode_literal_with_name_ref(name_match_index, value.as_bytes(), buf),
    }
}

fn encode_by_lookup(lookup: StaticLookup, name: &[u8], value: &[u8], buf: &mut Vec<u8>) {
    match lookup {
        StaticLookup::FullMatch(index) => encode_indexed(index, buf),
        StaticLookup::NameMatch(index) => encode_literal_with_name_ref(index, value, buf),
        StaticLookup::NoMatch => encode_literal_with_literal_name(name, value, buf),
    }
}

/// Encode a string literal per RFC 9204 §4.1.2.
///
/// Tries Huffman encoding and uses it when shorter. The H flag is
/// placed at bit `prefix_size` of the first byte; the length occupies
/// the low `prefix_size` bits as a varint. The caller is responsible
/// for OR-ing any additional flags into `buf[start]` after this returns.
fn encode_string(value: &[u8], prefix_size: u8, buf: &mut Vec<u8>) {
    let huffman_encoded = huffman::encode(value);
    let (bytes, huffman_flag) = if huffman_encoded.len() < value.len() {
        (huffman_encoded.as_slice(), 1_u8 << prefix_size)
    } else {
        (value, 0)
    };

    let start = buf.len();
    buf.extend_from_slice(&varint::encode(bytes.len(), prefix_size));
    buf[start] |= huffman_flag;
    buf.extend_from_slice(bytes);
}
