use compact_str::CompactString;
use std::{
    borrow::{Borrow, Cow},
    fmt::{Debug, Display, Formatter, Result},
    hash::{Hash, Hasher},
    ops::Deref,
    sync::Arc,
};

/// A cow that holds a borrowed `&str`, an owned [`CompactString`] (so short runtime strings
/// stay inline rather than hitting the heap), or a reference-counted `str` shared with
/// another owner such as a compression dynamic table.
#[derive(Clone)]
pub(crate) enum CompactCow<'a> {
    Borrowed(&'a str),
    Owned(CompactString),
    Shared(Arc<str>),
}

impl CompactCow<'_> {
    pub(crate) fn into_owned(self) -> CompactCow<'static> {
        match self {
            CompactCow::Borrowed(b) => CompactCow::Owned(CompactString::from(b)),
            CompactCow::Owned(o) => CompactCow::Owned(o),
            CompactCow::Shared(s) => CompactCow::Shared(s),
        }
    }
}

impl Default for CompactCow<'_> {
    fn default() -> Self {
        Self::Borrowed("")
    }
}

impl PartialEq<CompactCow<'_>> for CompactCow<'_> {
    fn eq(&self, other: &CompactCow<'_>) -> bool {
        **self == **other
    }
}

impl Eq for CompactCow<'_> {}

impl PartialEq<&str> for CompactCow<'_> {
    fn eq(&self, other: &&str) -> bool {
        &**self == *other
    }
}

impl Debug for CompactCow<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        Debug::fmt(&**self, f)
    }
}

impl Display for CompactCow<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_str(self)
    }
}

impl AsRef<str> for CompactCow<'_> {
    fn as_ref(&self) -> &str {
        self
    }
}

impl AsRef<[u8]> for CompactCow<'_> {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl<'a> From<&'a str> for CompactCow<'a> {
    fn from(s: &'a str) -> Self {
        Self::Borrowed(s)
    }
}

impl<'a> From<Cow<'a, str>> for CompactCow<'a> {
    fn from(s: Cow<'a, str>) -> Self {
        match s {
            Cow::Owned(string) => Self::Owned(CompactString::from(string)),
            Cow::Borrowed(s) => Self::Borrowed(s),
        }
    }
}

impl From<String> for CompactCow<'_> {
    fn from(s: String) -> Self {
        Self::Owned(CompactString::from(s))
    }
}

impl From<CompactString> for CompactCow<'_> {
    fn from(s: CompactString) -> Self {
        Self::Owned(s)
    }
}

impl From<Arc<str>> for CompactCow<'_> {
    fn from(s: Arc<str>) -> Self {
        Self::Shared(s)
    }
}

impl Deref for CompactCow<'_> {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Borrowed(b) => b,
            Self::Owned(o) => o,
            Self::Shared(s) => s,
        }
    }
}

impl Borrow<str> for CompactCow<'_> {
    fn borrow(&self) -> &str {
        self
    }
}

impl Hash for CompactCow<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.deref().hash(state);
    }
}
