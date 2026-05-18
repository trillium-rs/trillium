use encoding_rs::Encoding;
use mime::Mime;
use std::str::FromStr;
use trillium_http::{Headers, KnownHeaderName::ContentType};

/// Extract the character encoding declared by the `Content-Type` charset parameter, falling
/// back to WINDOWS-1252 when missing or unrecognized.
pub fn encoding(headers: &Headers) -> &'static Encoding {
    headers
        .get_str(ContentType)
        .and_then(|c| Mime::from_str(c).ok())
        .and_then(|m| {
            m.params()
                .find(|(name, _)| name.as_str() == "charset")
                .and_then(|(_, v)| Encoding::for_label(v.as_str().as_bytes()))
        })
        .unwrap_or(encoding_rs::WINDOWS_1252)
}
