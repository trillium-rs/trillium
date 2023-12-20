use smallvec::SmallVec;
use smartcow::SmartCow;

use std::{
    borrow::Cow,
    fmt::{Debug, Display, Formatter},
};

/// A `HeaderValue` represents the right hand side of a single `name:
/// value` pair.
#[derive(Eq, PartialEq, Clone)]
pub struct HeaderValue(HeaderValueInner);

#[derive(Eq, PartialEq, Clone)]
pub(crate) enum HeaderValueInner {
    Utf8(SmartCow<'static>),
    Bytes(SmallVec<[u8; 32]>),
}

#[cfg(feature = "serde")]
impl serde::Serialize for HeaderValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match &self.0 {
            HeaderValueInner::Utf8(s) => serializer.serialize_str(s),
            HeaderValueInner::Bytes(bytes) => serializer.serialize_bytes(bytes),
        }
    }
}

impl Debug for HeaderValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            HeaderValueInner::Utf8(s) => Debug::fmt(s, f),
            HeaderValueInner::Bytes(b) => Debug::fmt(&String::from_utf8_lossy(b), f),
        }
    }
}

impl HeaderValue {
    /// Returns this header value as a &str if it is utf8, None
    /// otherwise. If you need to convert non-utf8 bytes to a string
    /// somehow, match directly on the `HeaderValue` as an enum and
    /// handle that case. If you need a byte slice regardless of
    /// whether it's utf8, use the `AsRef<[u8]>` impl
    pub fn as_str(&self) -> Option<&str> {
        match &self.0 {
            HeaderValueInner::Utf8(utf8) => Some(utf8),
            HeaderValueInner::Bytes(_) => None,
        }
    }

    pub(crate) fn as_lower(&self) -> Option<SmartCow<'_>> {
        self.as_str().map(|s| {
            if s.chars().all(|c| c.is_ascii_lowercase()) {
                SmartCow::Borrowed(s)
            } else {
                SmartCow::Owned(s.chars().map(|c| c.to_ascii_lowercase()).collect())
            }
        })
    }
}

impl Display for HeaderValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            HeaderValueInner::Utf8(s) => f.write_str(s),
            HeaderValueInner::Bytes(b) => f.write_str(&String::from_utf8_lossy(b)),
        }
    }
}

impl From<Vec<u8>> for HeaderValue {
    fn from(v: Vec<u8>) -> Self {
        match String::from_utf8(v) {
            Ok(s) => Self(HeaderValueInner::Utf8(SmartCow::Owned(s.into()))),
            Err(e) => Self(HeaderValueInner::Bytes(e.into_bytes().into())),
        }
    }
}

impl From<Cow<'static, str>> for HeaderValue {
    fn from(c: Cow<'static, str>) -> Self {
        Self(HeaderValueInner::Utf8(SmartCow::from(c)))
    }
}

impl From<&'static [u8]> for HeaderValue {
    fn from(b: &'static [u8]) -> Self {
        match std::str::from_utf8(b) {
            Ok(s) => Self(HeaderValueInner::Utf8(SmartCow::Borrowed(s))),
            Err(_) => Self(HeaderValueInner::Bytes(b.into())),
        }
    }
}

impl From<String> for HeaderValue {
    fn from(s: String) -> Self {
        Self(HeaderValueInner::Utf8(SmartCow::Owned(s.into())))
    }
}

impl From<&'static str> for HeaderValue {
    fn from(s: &'static str) -> Self {
        Self(HeaderValueInner::Utf8(SmartCow::Borrowed(s)))
    }
}

impl AsRef<[u8]> for HeaderValue {
    fn as_ref(&self) -> &[u8] {
        match &self.0 {
            HeaderValueInner::Utf8(utf8) => utf8.as_bytes(),
            HeaderValueInner::Bytes(b) => b,
        }
    }
}

impl PartialEq<&str> for HeaderValue {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == Some(*other)
    }
}

impl PartialEq<&[u8]> for HeaderValue {
    fn eq(&self, other: &&[u8]) -> bool {
        self.as_ref() == *other
    }
}

impl PartialEq<[u8]> for HeaderValue {
    fn eq(&self, other: &[u8]) -> bool {
        self.as_ref() == other
    }
}

impl PartialEq<str> for HeaderValue {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == Some(other)
    }
}

impl PartialEq<String> for HeaderValue {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == Some(other)
    }
}

impl PartialEq<&String> for HeaderValue {
    fn eq(&self, other: &&String) -> bool {
        self.as_str() == Some(&**other)
    }
}

impl PartialEq<[u8]> for &HeaderValue {
    fn eq(&self, other: &[u8]) -> bool {
        self.as_ref() == other
    }
}

impl PartialEq<str> for &HeaderValue {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == Some(other)
    }
}

impl PartialEq<String> for &HeaderValue {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == Some(other)
    }
}
