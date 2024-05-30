use smallvec::SmallVec;
use smartcow::SmartCow;
use smartstring::SmartString;
use std::{
    borrow::Cow,
    fmt::{Debug, Display, Formatter, Write},
};
use HeaderValueInner::{Bytes, Utf8};

/// A `HeaderValue` represents the right hand side of a single `name:
/// value` pair.
#[derive(Eq, PartialEq, Clone, Ord, PartialOrd)]
pub struct HeaderValue(pub(crate) HeaderValueInner);

#[derive(Eq, PartialEq, Clone)]
pub(crate) enum HeaderValueInner {
    Utf8(SmartCow<'static>),
    Bytes(SmallVec<[u8; 32]>),
}

impl PartialOrd for HeaderValueInner {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for HeaderValueInner {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let this: &[u8] = self.as_ref();
        let that: &[u8] = other.as_ref();
        this.cmp(that)
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for HeaderValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match &self.0 {
            Utf8(s) => serializer.serialize_str(s),
            Bytes(bytes) => serializer.serialize_bytes(bytes),
        }
    }
}

impl Debug for HeaderValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Utf8(s) => Debug::fmt(s, f),
            Bytes(b) => Debug::fmt(&String::from_utf8_lossy(b), f),
        }
    }
}

impl HeaderValue {
    /// determine if this header contains no unsafe characters (\r, \n, \0)
    ///
    /// since 0.3.12
    pub fn is_valid(&self) -> bool {
        memchr::memchr3(b'\r', b'\n', 0, self.as_ref()).is_none()
    }

    /// Returns this header value as a &str if it is utf8, None
    /// otherwise. If you need to convert non-utf8 bytes to a string
    /// somehow, match directly on the `HeaderValue` as an enum and
    /// handle that case. If you need a byte slice regardless of
    /// whether it's utf8, use the `AsRef<[u8]>` impl
    pub fn as_str(&self) -> Option<&str> {
        match &self.0 {
            Utf8(utf8) => Some(utf8),
            Bytes(_) => None,
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

#[cfg(feature = "parse")]
impl HeaderValue {
    pub(crate) fn parse(bytes: &[u8]) -> Self {
        match std::str::from_utf8(bytes) {
            Ok(s) => Self(Utf8(SmartCow::Owned(s.into()))),
            Err(_) => Self(Bytes(bytes.into())),
        }
    }
}

impl Display for HeaderValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Utf8(s) => f.write_str(s),
            Bytes(b) => f.write_str(&String::from_utf8_lossy(b)),
        }
    }
}

impl From<Vec<u8>> for HeaderValue {
    fn from(v: Vec<u8>) -> Self {
        match String::from_utf8(v) {
            Ok(s) => Self(Utf8(SmartCow::Owned(s.into()))),
            Err(e) => Self(Bytes(e.into_bytes().into())),
        }
    }
}

impl From<Cow<'static, str>> for HeaderValue {
    fn from(c: Cow<'static, str>) -> Self {
        Self(Utf8(SmartCow::from(c)))
    }
}

impl From<&'static [u8]> for HeaderValue {
    fn from(b: &'static [u8]) -> Self {
        match std::str::from_utf8(b) {
            Ok(s) => Self(Utf8(SmartCow::Borrowed(s))),
            Err(_) => Self(Bytes(b.into())),
        }
    }
}

impl From<String> for HeaderValue {
    fn from(s: String) -> Self {
        Self(Utf8(SmartCow::Owned(s.into())))
    }
}

impl From<&'static str> for HeaderValue {
    fn from(s: &'static str) -> Self {
        Self(Utf8(SmartCow::Borrowed(s)))
    }
}

macro_rules! delegate_from_to_format {
    ($($t:ty),*) => {
        $(
        impl From<$t> for HeaderValue {
            fn from(value: $t) -> Self {
                format_args!("{value}").into()
            }
        }
        )*
    };
}

delegate_from_to_format!(usize, u64, u16, u32, i32, i64);

impl From<std::fmt::Arguments<'_>> for HeaderValue {
    fn from(value: std::fmt::Arguments<'_>) -> Self {
        let mut s = SmartString::new();
        s.write_fmt(value).unwrap();
        Self(Utf8(SmartCow::Owned(s)))
    }
}

impl AsRef<[u8]> for HeaderValueInner {
    fn as_ref(&self) -> &[u8] {
        match self {
            Utf8(utf8) => utf8.as_bytes(),
            Bytes(b) => b,
        }
    }
}

impl AsRef<[u8]> for HeaderValue {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
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
