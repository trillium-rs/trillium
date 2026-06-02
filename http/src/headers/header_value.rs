use HeaderValueInner::{Bytes, Utf8};
use smartcow::SmartCow;
use smartstring::SmartString;
use std::{
    borrow::Cow,
    fmt::{Debug, Display, Formatter, Write},
};

/// A `HeaderValue` represents the right hand side of a single `name:
/// value` pair.
#[derive(Clone)]
pub struct HeaderValue {
    pub(crate) inner: HeaderValueInner,
    /// HPACK / QPACK "Never-Indexed" bit, carried for proxy round-trip fidelity:
    /// decoders set it from the wire bit and encoders re-emit a never-indexed
    /// literal when it is set. Not part of value identity (`PartialEq` / `Hash`
    /// ignore it).
    pub(crate) never_indexed: bool,
}

impl HeaderValue {
    pub(crate) const fn from_inner(inner: HeaderValueInner) -> Self {
        Self {
            inner,
            never_indexed: false,
        }
    }

    pub(crate) fn is_never_indexed(&self) -> bool {
        self.never_indexed
    }

    pub(crate) fn set_never_indexed(&mut self, never_indexed: bool) {
        self.never_indexed = never_indexed;
    }
}

impl PartialEq for HeaderValue {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl Eq for HeaderValue {}

impl std::hash::Hash for HeaderValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.inner.hash(state);
    }
}

impl PartialOrd for HeaderValue {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeaderValue {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.inner.cmp(&other.inner)
    }
}

impl From<Cow<'static, [u8]>> for HeaderValue {
    fn from(value: Cow<'static, [u8]>) -> Self {
        match value {
            Cow::Borrowed(bytes) => match std::str::from_utf8(bytes) {
                Ok(s) => Self::from_inner(Utf8(SmartCow::Borrowed(s))),
                Err(_) => Self::from_inner(Bytes(bytes.into())),
            },

            Cow::Owned(bytes) => match String::from_utf8(bytes) {
                Ok(s) => Self::from_inner(Utf8(SmartCow::Owned(s.into()))),
                Err(e) => Self::from_inner(Bytes(e.into_bytes().into())),
            },
        }
    }
}

#[derive(Eq, PartialEq, Clone, Hash)]
pub(crate) enum HeaderValueInner {
    Utf8(SmartCow<'static>),
    Bytes(Box<[u8]>),
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
        match &self.inner {
            Utf8(s) => serializer.serialize_str(s),
            Bytes(bytes) => serializer.serialize_bytes(bytes),
        }
    }
}

impl Debug for HeaderValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            Utf8(s) => Debug::fmt(s, f),
            Bytes(b) => Debug::fmt(&String::from_utf8_lossy(b), f),
        }
    }
}

impl HeaderValue {
    /// Build a new header value from a &'static str at compile time
    pub const fn const_new(value: &'static str) -> Self {
        Self::from_inner(Utf8(SmartCow::Borrowed(value)))
    }

    /// determine if this header contains no unsafe characters (\r, \n, \0)
    pub fn is_valid(&self) -> bool {
        memchr::memchr3(b'\r', b'\n', 0, self.as_ref()).is_none()
    }

    /// Returns this header value as a &str if it is utf8, `None` otherwise. For
    /// a byte slice regardless of utf8-ness, use the `AsRef<[u8]>` impl.
    pub fn as_str(&self) -> Option<&str> {
        match &self.inner {
            Utf8(utf8) => Some(utf8),
            Bytes(_) => None,
        }
    }
}

#[cfg(feature = "parse")]
impl HeaderValue {
    pub(crate) fn parse(bytes: &[u8]) -> Self {
        match std::str::from_utf8(bytes) {
            Ok(s) => Self::from_inner(Utf8(SmartCow::Owned(s.into()))),
            Err(_) => Self::from_inner(Bytes(bytes.into())),
        }
    }
}

impl Display for HeaderValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            Utf8(s) => f.write_str(s),
            Bytes(b) => f.write_str(&String::from_utf8_lossy(b)),
        }
    }
}

impl From<Vec<u8>> for HeaderValue {
    fn from(v: Vec<u8>) -> Self {
        match String::from_utf8(v) {
            Ok(s) => Self::from_inner(Utf8(SmartCow::Owned(s.into()))),
            Err(e) => Self::from_inner(Bytes(e.into_bytes().into())),
        }
    }
}

impl From<Cow<'static, str>> for HeaderValue {
    fn from(c: Cow<'static, str>) -> Self {
        Self::from_inner(Utf8(SmartCow::from(c)))
    }
}

impl From<&'static [u8]> for HeaderValue {
    fn from(b: &'static [u8]) -> Self {
        match std::str::from_utf8(b) {
            Ok(s) => Self::from_inner(Utf8(SmartCow::Borrowed(s))),
            Err(_) => Self::from_inner(Bytes(b.into())),
        }
    }
}

impl From<String> for HeaderValue {
    fn from(s: String) -> Self {
        Self::from_inner(Utf8(SmartCow::Owned(s.into())))
    }
}

impl From<&'static str> for HeaderValue {
    fn from(s: &'static str) -> Self {
        Self::from_inner(Utf8(SmartCow::Borrowed(s)))
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
        Self::from_inner(Utf8(SmartCow::Owned(s)))
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
        self.inner.as_ref()
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
