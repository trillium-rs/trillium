use crate::{Error, HeaderValues, Headers, KnownHeaderName, Result};
use encoding_rs::Encoding;
use mime::Mime;
use std::str::FromStr;

/// The character encoding declared by the `Content-Type` header's `charset` parameter, or
/// UTF-8 if absent or unparseable.
pub fn encoding(headers: &Headers) -> &'static Encoding {
    headers
        .get_str(KnownHeaderName::ContentType)
        .and_then(|c| Mime::from_str(c).ok())
        .and_then(|m| {
            m.params()
                .find(|(name, _)| name.as_str() == "charset")
                .and_then(|(_, v)| Encoding::for_label(v.as_str().as_bytes()))
        })
        .unwrap_or(encoding_rs::UTF_8)
}

/// Validate and parse an HTTP/1.x `Content-Length` per RFC 9110.
///
/// `values` is the header's value set (`None` if absent). Returns `Ok(None)` when absent and
/// `Ok(Some(len))` for exactly one value that is a non-empty run of ASCII digits fitting in a
/// `u64`.
///
/// Coercing a malformed `Content-Length` to a default body length (zero, or read-to-close) rather
/// than rejecting it is a request/response-smuggling vector, so the server request parser and the
/// client response parser both route through this single check to stay byte-for-byte identical.
///
/// # Errors
///
/// Returns `Err(InvalidHeaderValue)` for more than one value, an empty value, any non-digit octet
/// (notably a leading `+`/`-`, which `u64::from_str` would otherwise accept), or a value that
/// overflows `u64`.
pub fn validate_content_length(values: Option<&HeaderValues>) -> Result<Option<u64>> {
    match values {
        None => Ok(None),
        Some(values) => parse_content_length(values)
            .map(Some)
            .ok_or_else(|| Error::InvalidHeaderValue(KnownHeaderName::ContentLength.into())),
    }
}

/// Parse a `Content-Length` value set into a `u64`, returning `None` for a multi-value set, an
/// empty value, any non-digit octet (notably a leading `+`/`-`, which `u64::from_str` would
/// otherwise accept), or a value that overflows `u64`. Shared by [`validate_content_length`] (the
/// strict ingress gate, which turns this `None` into an error) and [`Headers::content_length`]
/// (the downstream read, which surfaces it directly).
///
/// [`Headers::content_length`]: crate::Headers::content_length
pub(crate) fn parse_content_length(values: &HeaderValues) -> Option<u64> {
    values
        .as_str()
        .filter(|cl| !cl.is_empty() && cl.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|cl| cl.parse::<u64>().ok())
}

/// Whether `b` is a `tchar` — the octets permitted in a `token` (method names, header names,
/// chunk-extension names, etc.).
pub(crate) fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}
