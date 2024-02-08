// originally from https://github.com/http-rs/http-types/blob/main/src/version.rs

use std::{
    fmt::Display,
    str::{self, FromStr},
};

use crate::Error;

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

#[cfg(feature = "serde")]
impl serde::Serialize for Version {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Version {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl PartialEq<&Version> for Version {
    #[allow(clippy::unconditional_recursion)] // false positive
    fn eq(&self, other: &&Version) -> bool {
        self == *other
    }
}

impl PartialEq<Version> for &Version {
    #[allow(clippy::unconditional_recursion)] // false positive
    fn eq(&self, other: &Version) -> bool {
        *self == other
    }
}

impl Version {
    /// returns the http version as a static str, such as "HTTP/1.1"
    pub const fn as_str(&self) -> &'static str {
        match self {
            Version::Http0_9 => "HTTP/0.9",
            Version::Http1_0 => "HTTP/1.0",
            Version::Http1_1 => "HTTP/1.1",
            Version::Http2_0 => "HTTP/2",
            Version::Http3_0 => "HTTP/3",
        }
    }

    #[cfg(feature = "parse")]
    pub(crate) fn parse(buf: &[u8]) -> crate::Result<Self> {
        str::from_utf8(buf)
            .map_err(|_| Error::InvalidVersion)?
            .parse()
    }
}

impl FromStr for Version {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "HTTP/0.9" | "http/0.9" | "0.9" => Ok(Self::Http0_9),
            "HTTP/1.0" | "http/1.0" | "1.0" => Ok(Self::Http1_0),
            "HTTP/1.1" | "http/1.1" | "1.1" => Ok(Self::Http1_1),
            "HTTP/2" | "http/2" | "2" => Ok(Self::Http2_0),
            "HTTP/3" | "http/3" | "3" => Ok(Self::Http3_0),
            _ => Err(Error::InvalidVersion),
        }
    }
}

impl AsRef<str> for Version {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<[u8]> for Version {
    fn as_ref(&self) -> &[u8] {
        self.as_str().as_bytes()
    }
}

impl Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_ref())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn from_str() {
        let versions = [
            Version::Http0_9,
            Version::Http1_0,
            Version::Http1_1,
            Version::Http2_0,
            Version::Http3_0,
        ];

        for version in versions {
            assert_eq!(version.as_str().parse::<Version>().unwrap(), version);
            assert_eq!(version.to_string().parse::<Version>().unwrap(), version);
        }

        assert_eq!(
            "not a version".parse::<Version>().unwrap_err().to_string(),
            "Invalid or missing version"
        );
    }

    #[test]
    fn eq() {
        assert_eq!(Version::Http1_1, Version::Http1_1);
        assert_eq!(Version::Http1_1, &Version::Http1_1);
        assert_eq!(&Version::Http1_1, Version::Http1_1);
    }

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
        use Version::{Http0_9, Http1_0, Http1_1, Http2_0, Http3_0};
        assert!(Http3_0 > Http2_0);
        assert!(Http2_0 > Http1_1);
        assert!(Http1_1 > Http1_0);
        assert!(Http1_0 > Http0_9);
    }
}
