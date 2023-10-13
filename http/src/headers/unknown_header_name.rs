use super::{weakrange::WeakRange, HeaderName, HeaderNameInner::UnknownHeader};
use hashbrown::Equivalent;
use smartcow::SmartCow;
use smartstring::alias::String as SmartString;
use std::{
    fmt::{self, Debug, Display, Formatter},
    hash::{Hash, Hasher},
    marker::PhantomData,
    ops::Deref,
    sync::Weak,
};

#[derive(Clone)]
pub(super) struct UnknownHeaderName(SmartString);
impl PartialEq for UnknownHeaderName {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref().eq_ignore_ascii_case(other.as_ref())
    }
}

impl AsRef<str> for UnknownHeaderName {
    fn as_ref(&self) -> &str {
        &*self.0
    }
}

impl Eq for UnknownHeaderName {}

impl Hash for UnknownHeaderName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for c in self.as_ref().as_bytes() {
            c.to_ascii_lowercase().hash(state);
        }
    }
}

impl Debug for UnknownHeaderName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(self.as_ref(), f)
    }
}

impl Display for UnknownHeaderName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self.as_ref(), f)
    }
}

impl From<SmartString> for UnknownHeaderName {
    fn from(value: SmartString) -> Self {
        Self(value)
    }
}

impl Deref for UnknownHeaderName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl Equivalent<UnknownHeaderName> for &UnknownHeaderName {
    fn equivalent(&self, key: &UnknownHeaderName) -> bool {
        key.eq_ignore_ascii_case(self)
    }
}
