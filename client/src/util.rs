use encoding_rs::Encoding;
use mime::Mime;
use std::str::FromStr;
use trillium_http::{Headers, KnownHeaderName::ContentType};

/// a utility function for extracting a character encoding from a set
/// of [`Headers`][trillium::Headers]
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
