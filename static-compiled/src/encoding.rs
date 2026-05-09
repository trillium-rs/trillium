/// A precompressed content coding baked into the binary at compile time.
///
/// Variants correspond to the [HTTP content-coding tokens][content-coding]
/// returned by [`Encoding::token`] and used in the `Content-Encoding`
/// response header.
///
/// [content-coding]: https://www.rfc-editor.org/rfc/rfc9110.html#name-content-codings
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Encoding {
    /// Brotli (`br`).
    Brotli,
    /// Zstandard (`zstd`).
    Zstd,
    /// Gzip (`gzip`).
    Gzip,
}

impl Encoding {
    /// The HTTP content-coding token for this encoding.
    pub const fn token(self) -> &'static str {
        match self {
            Self::Brotli => "br",
            Self::Zstd => "zstd",
            Self::Gzip => "gzip",
        }
    }
}

/// Returns true if `accept` (an `Accept-Encoding` header value) permits the
/// given encoding token. Honors q-values and the `*` wildcard, with explicit
/// named tokens overriding the wildcard.
pub(crate) fn accept_encoding_allows(accept: &str, encoding: &str) -> bool {
    let mut wildcard_ok = false;
    let mut named_ok = None;
    for part in accept.split(',') {
        let mut iter = part.trim().split(';');
        let token = iter.next().unwrap_or("").trim();
        let q = iter
            .find_map(|p| {
                p.trim()
                    .strip_prefix("q=")
                    .and_then(|q| q.parse::<f32>().ok())
            })
            .unwrap_or(1.0);
        if token.eq_ignore_ascii_case(encoding) {
            named_ok = Some(q > 0.0);
        } else if token == "*" && named_ok.is_none() {
            wildcard_ok = q > 0.0;
        }
    }
    named_ok.unwrap_or(wildcard_ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_round_trip() {
        assert_eq!(Encoding::Brotli.token(), "br");
        assert_eq!(Encoding::Zstd.token(), "zstd");
        assert_eq!(Encoding::Gzip.token(), "gzip");
    }

    #[test]
    fn accept_encoding_basics() {
        assert!(accept_encoding_allows("br, gzip", "br"));
        assert!(accept_encoding_allows("br, gzip", "gzip"));
        assert!(!accept_encoding_allows("br, gzip", "zstd"));
    }

    #[test]
    fn accept_encoding_q_values() {
        assert!(accept_encoding_allows("br;q=0.5, gzip", "br"));
        assert!(!accept_encoding_allows("br;q=0, gzip", "br"));
        assert!(accept_encoding_allows("br;q=0, gzip", "gzip"));
    }

    #[test]
    fn accept_encoding_wildcard() {
        assert!(accept_encoding_allows("*", "br"));
        assert!(!accept_encoding_allows("*;q=0", "br"));
        assert!(accept_encoding_allows("*, br;q=0", "gzip"));
        assert!(!accept_encoding_allows("*, br;q=0", "br"));
    }

    #[test]
    fn accept_encoding_case_insensitive() {
        assert!(accept_encoding_allows("BR, GZIP", "br"));
        assert!(accept_encoding_allows("Br", "br"));
    }
}
