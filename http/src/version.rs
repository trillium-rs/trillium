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
mod serde {
    use super::Version;
    use serde_crate::{
        de::{Error, Unexpected, Visitor},
        Deserialize, Deserializer, Serialize, Serializer,
    };

    impl Serialize for Version {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_str(&self.to_string())
        }
    }

    struct VersionVisitor;

    impl<'de> Visitor<'de> for VersionVisitor {
        type Value = Version;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(formatter, "a HTTP version as &str")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: Error,
        {
            match v {
                "HTTP/0.9" => Ok(Version::Http0_9),
                "HTTP/1.0" => Ok(Version::Http1_0),
                "HTTP/1.1" => Ok(Version::Http1_1),
                "HTTP/2" => Ok(Version::Http2_0),
                "HTTP/3" => Ok(Version::Http3_0),
                _ => Err(Error::invalid_value(Unexpected::Str(v), &self)),
            }
        }

        fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
        where
            E: Error,
        {
            self.visit_str(&v)
        }
    }

    impl<'de> Deserialize<'de> for Version {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_str(VersionVisitor)
        }
    }
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

    #[cfg(feature = "serde")]
    #[test]
    fn serde() -> Result<(), serde_json::Error> {
        assert_eq!("\"HTTP/3\"", serde_json::to_string(&Version::Http3_0)?);
        assert_eq!(Version::Http1_1, serde_json::from_str("\"HTTP/1.1\"")?);
        Ok(())
    }
}
