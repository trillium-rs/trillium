use smallvec::SmallVec;
use smartcow::SmartCow;
use smartstring::alias::String as SmartString;
use std::fmt::{Debug, Display, Formatter};

#[derive(Eq, PartialEq, Clone)]
pub enum HeaderValue {
    Utf8(SmartCow<'static>),
    Bytes(SmallVec<[u8; 32]>),
}

impl Debug for HeaderValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            HeaderValue::Utf8(s) => Debug::fmt(s, f),
            HeaderValue::Bytes(b) => Debug::fmt(&String::from_utf8_lossy(b), f),
        }
    }
}

impl HeaderValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            HeaderValue::Utf8(utf8) => Some(&*utf8),
            HeaderValue::Bytes(_) => None,
        }
    }

    pub fn as_lower(&self) -> Option<SmartCow<'_>> {
        self.as_str().map(|s| {
            if s.is_ascii() {
                SmartCow::Borrowed(s)
            } else {
                SmartCow::Owned(s.to_ascii_lowercase().into())
            }
        })
    }
}

impl AsRef<[u8]> for HeaderValue {
    fn as_ref(&self) -> &[u8] {
        match self {
            HeaderValue::Utf8(utf8) => utf8.as_bytes(),
            HeaderValue::Bytes(b) => &*b,
        }
    }
}

impl From<Vec<u8>> for HeaderValue {
    fn from(v: Vec<u8>) -> Self {
        match String::from_utf8(v) {
            Ok(s) => Self::Utf8(SmartCow::Owned(s.into())),
            Err(e) => Self::Bytes(e.into_bytes().into()),
        }
    }
}

impl Display for HeaderValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            HeaderValue::Utf8(s) => f.write_str(s),
            HeaderValue::Bytes(b) => f.write_str(&String::from_utf8_lossy(b)),
        }
    }
}

impl From<String> for HeaderValue {
    fn from(s: String) -> Self {
        Self::Utf8(SmartCow::Owned(s.into()))
    }
}

impl From<&'static str> for HeaderValue {
    fn from(s: &'static str) -> Self {
        Self::Utf8(SmartCow::Borrowed(s))
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
        self.as_str() == Some(&*other)
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
        self.as_str() == Some(&*other)
    }
}
