// originally from https://github.com/http-rs/http-types/blob/main/src/version.rs

/// The version of the HTTP protocol in use.
#[derive(Copy, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum Version {
    /// HTTP/0.9
    Http0_9,

    /// HTTP/1.0
    Http1_0,

    /// HTTP/1.1
    Http1_1,

    /// HTTP/2.0
    Http2_0,

    /// HTTP/3.0
    Http3_0,
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Version::Http0_9 => "HTTP/0.9",
            Version::Http1_0 => "HTTP/1.0",
            Version::Http1_1 => "HTTP/1.1",
            Version::Http2_0 => "HTTP/2",
            Version::Http3_0 => "HTTP/3",
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn to_string() {
        let output = format!(
            "{} {} {} {} {}",
            Version::Http0_9,
            Version::Http1_0,
            Version::Http1_1,
            Version::Http2_0,
            Version::Http3_0
        );
        assert_eq!("HTTP/0.9 HTTP/1.0 HTTP/1.1 HTTP/2 HTTP/3", output);
    }

    #[test]
    fn ord() {
        use Version::*;
        assert!(Http3_0 > Http2_0);
        assert!(Http2_0 > Http1_1);
        assert!(Http1_1 > Http1_0);
        assert!(Http1_0 > Http0_9);
    }
}
