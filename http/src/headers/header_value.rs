use super::weakrange::WeakRange;
use smallvec::SmallVec;
use smartcow::SmartCow;
use smartstring::alias::String as SmartString;
use std::{
    borrow::Cow,
    fmt::{Debug, Display, Formatter},
    sync::Weak,
};

/// A `HeaderValue` represents the right hand side of a single `name:
/// value` pair.
#[derive(Eq, PartialEq, Clone)]
pub struct HeaderValue(HeaderValueInner);

#[derive(Eq, PartialEq)]
pub(crate) enum HeaderValueInner {
    OwnedUtf8(SmartString),
    BorrowedUtf8(WeakRange<str>),
    OwnedBytes(SmallVec<[u8; 32]>),
    BorrowedBytes(WeakRange<[u8]>),
    StaticStr(&'static str),
    StaticBytes(&'static [u8]),
}

impl Clone for HeaderValueInner {
    fn clone(&self) -> Self {
        match self {
            HeaderValueInner::OwnedUtf8(o) => HeaderValueInner::OwnedUtf8(o.clone()),
            HeaderValueInner::BorrowedUtf8(b) => HeaderValueInner::OwnedUtf8(b.as_ref().into()),
            HeaderValueInner::OwnedBytes(o) => HeaderValueInner::OwnedBytes(o.clone()),
            HeaderValueInner::BorrowedBytes(b) => HeaderValueInner::OwnedBytes(b.as_ref().into()),
            HeaderValueInner::StaticStr(s) => HeaderValueInner::StaticStr(s),
            HeaderValueInner::StaticBytes(s) => HeaderValueInner::StaticBytes(s),
        }
    }
}

impl From<HeaderValueInner> for HeaderValue {
    fn from(value: HeaderValueInner) -> Self {
        HeaderValue(value)
    }
}

impl Debug for HeaderValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            HeaderValueInner::OwnedUtf8(s) => Debug::fmt(s, f),
            HeaderValueInner::OwnedBytes(b) => Debug::fmt(&String::from_utf8_lossy(b), f),
            HeaderValueInner::BorrowedUtf8(w) => Debug::fmt(&*w, f),
            HeaderValueInner::BorrowedBytes(w) => Debug::fmt(&String::from_utf8_lossy(w), f),
            HeaderValueInner::StaticStr(s) => Debug::fmt(s, f),
            HeaderValueInner::StaticBytes(s) => Debug::fmt(&String::from_utf8_lossy(s), f),
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
            HeaderValueInner::OwnedUtf8(s) => Some(s.as_str()),
            HeaderValueInner::BorrowedUtf8(s) => Some(s.as_ref()),
            HeaderValueInner::StaticStr(s) => Some(*s),
            _ => None,
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
            HeaderValueInner::OwnedUtf8(s) => Display::fmt(s, f),
            HeaderValueInner::OwnedBytes(b) => Display::fmt(&String::from_utf8_lossy(b), f),
            HeaderValueInner::BorrowedUtf8(s) => Display::fmt(&*s, f),
            HeaderValueInner::BorrowedBytes(b) => Display::fmt(&String::from_utf8_lossy(&b), f),
            HeaderValueInner::StaticStr(s) => Display::fmt(s, f),
            HeaderValueInner::StaticBytes(b) => Display::fmt(&String::from_utf8_lossy(&b), f),
        }
    }
}

impl From<Vec<u8>> for HeaderValue {
    fn from(v: Vec<u8>) -> Self {
        match String::from_utf8(v) {
            Ok(s) => Self(HeaderValueInner::OwnedUtf8(s.into())),
            Err(e) => Self(HeaderValueInner::OwnedBytes(e.into_bytes().into())),
        }
    }
}

impl From<Cow<'static, str>> for HeaderValue {
    fn from(c: Cow<'static, str>) -> Self {
        match c {
            Cow::Borrowed(c) => c.into(),
            Cow::Owned(o) => o.into(),
        }
    }
}

impl From<&'static [u8]> for HeaderValue {
    fn from(b: &'static [u8]) -> Self {
        match std::str::from_utf8(b) {
            Ok(s) => s.into(),
            Err(_) => Self(HeaderValueInner::StaticBytes(b.into())),
        }
    }
}

impl From<String> for HeaderValue {
    fn from(s: String) -> Self {
        Self(HeaderValueInner::OwnedUtf8(s.into()))
    }
}

impl From<&'static str> for HeaderValue {
    fn from(s: &'static str) -> Self {
        Self(HeaderValueInner::StaticStr(s))
    }
}

impl AsRef<[u8]> for HeaderValue {
    fn as_ref(&self) -> &[u8] {
        match &self.0 {
            HeaderValueInner::OwnedUtf8(s) => s.as_bytes(),
            HeaderValueInner::BorrowedUtf8(s) => s.as_bytes(),
            HeaderValueInner::OwnedBytes(o) => o.as_ref(),
            HeaderValueInner::BorrowedBytes(b) => b,
            HeaderValueInner::StaticStr(s) => s.as_bytes(),
            HeaderValueInner::StaticBytes(b) => b,
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
