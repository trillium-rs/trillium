use crate::{Headers, KnownHeaderName};
use encoding_rs::Encoding;
use mime::Mime;
use std::str::FromStr;

/// The character encoding declared by the `Content-Type` header's `charset` parameter, or
/// Windows-1252 if absent or unparseable.
pub fn encoding(headers: &Headers) -> &'static Encoding {
    headers
        .get_str(KnownHeaderName::ContentType)
        .and_then(|c| Mime::from_str(c).ok())
        .and_then(|m| {
            m.params()
                .find(|(name, _)| name.as_str() == "charset")
                .and_then(|(_, v)| Encoding::for_label(v.as_str().as_bytes()))
        })
        .unwrap_or(encoding_rs::WINDOWS_1252)
}

/// Whether `b` is an RFC 9110 §5.6.2 `tchar` — the octets permitted in a `token` (method
/// names, header names, chunk-extension names, etc.).
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
