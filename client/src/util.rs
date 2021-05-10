use encoding_rs::Encoding;
use std::str::FromStr;
use trillium_http::http_types::headers::{Headers, CONTENT_TYPE};
use trillium_http::http_types::mime::Mime;

/// a utility function for extracting a character encoding from a set
/// of [`Headers`][http_types::headers::Headers]
pub fn encoding(headers: &Headers) -> &'static Encoding {
    let mime = headers
        .get(CONTENT_TYPE)
        .and_then(|c| Mime::from_str(c.as_str()).ok());

    mime.and_then(|m| {
        m.param("charset")
            .and_then(|v| Encoding::for_label(v.as_str().as_bytes()))
    })
    .unwrap_or(encoding_rs::WINDOWS_1252)
}
