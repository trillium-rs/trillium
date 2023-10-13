use super::{KnownHeaderName, UnknownHeaderName};
use crate::Error;
use smartstring::alias::String as SmartString;
use std::{
    borrow::Cow,
    fmt::{self, Debug, Display, Formatter},
    hash::Hash,
    str::FromStr,
    sync::{Arc, Weak},
};
use HeaderNameInner::{KnownHeader, UnknownHeader};

/// The name of a http header. This can be either a
/// [`KnownHeaderName`] or a string representation of an unknown
/// header.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HeaderName<'a>(pub(super) HeaderNameInner<'a>);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum HeaderNameInner<'a> {
    /// A `KnownHeaderName`
    KnownHeader(KnownHeaderName),
    UnknownHeader(Cow<'a, UnknownHeaderName>),
}

impl<'a> From<&'a UnknownHeaderName> for HeaderName<'a> {
    fn from(value: &'a UnknownHeaderName) -> Self {
        Self(UnknownHeader(Cow::Borrowed(value)))
    }
}

impl<'a> From<&'a KnownHeaderName> for HeaderName<'_> {
    fn from(value: &'a KnownHeaderName) -> Self {
        Self(KnownHeader(*value))
    }
}

impl From<UnknownHeaderName> for HeaderName<'_> {
    fn from(value: UnknownHeaderName) -> Self {
        Self(UnknownHeader(Cow::Owned(value.into_owned())))
    }
}

impl HeaderName<'_> {
    /// Convert a potentially-borrowed headername to a static
    /// headername _by value_.
    #[must_use]
    pub fn into_owned(self) -> HeaderName<'static> {
        HeaderName(match self.0 {
            KnownHeader(known) => KnownHeader(known),
            UnknownHeader(Cow::Owned(o)) => UnknownHeader(Cow::Owned(o.into_owned())),
            UnknownHeader(Cow::Borrowed(o)) => UnknownHeader(Cow::Owned(o.clone())),
        })
    }

    /// Convert a potentially-borrowed headername to a static
    /// headername _by cloning if needed from a borrow_. If you have
    /// ownership of a headername with a non-static lifetime, it is
    /// preferable to use `into_owned`. This is the equivalent of
    /// `self.clone().into_owned()`.
    #[must_use]
    pub fn to_owned(&self) -> HeaderName<'static> {
        self.clone().into_owned()
    }
}

impl PartialEq<KnownHeaderName> for HeaderName<'_> {
    fn eq(&self, other: &KnownHeaderName) -> bool {
        match &self.0 {
            KnownHeader(k) => other == k,
            UnknownHeader(_) => false,
        }
    }
}

impl PartialEq<KnownHeaderName> for &HeaderName<'_> {
    fn eq(&self, other: &KnownHeaderName) -> bool {
        match &self.0 {
            KnownHeader(k) => other == k,
            UnknownHeader(_) => false,
        }
    }
}

impl From<String> for HeaderName<'_> {
    fn from(s: String) -> Self {
        Self(match s.parse::<KnownHeaderName>() {
            Ok(khn) => KnownHeader(khn),
            Err(()) => UnknownHeader(Cow::Owned(UnknownHeaderName::Owned(s.into()))),
        })
    }
}

impl From<&str> for HeaderName<'_> {
    fn from(s: &str) -> Self {
        Self(match s.parse::<KnownHeaderName>() {
            Ok(khn) => KnownHeader(khn),
            Err(_e) => UnknownHeader(Cow::Owned(UnknownHeaderName::Owned(SmartString::from(s)))),
        })
    }
}

impl FromStr for HeaderName<'_> {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_ascii() {
            Ok(Self(match s.parse::<KnownHeaderName>() {
                Ok(known) => KnownHeader(known),
                Err(_) => UnknownHeader(Cow::Owned(UnknownHeaderName::Owned(SmartString::from(s)))),
            }))
        } else {
            Err(Error::MalformedHeader(s.to_string().into()))
        }
    }
}

impl AsRef<str> for HeaderName<'_> {
    fn as_ref(&self) -> &str {
        match &self.0 {
            KnownHeader(khn) => khn.as_ref(),
            UnknownHeader(u) => u.as_ref(),
        }
    }
}

impl Display for HeaderName<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}
