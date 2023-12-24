use smartcow::SmartCow;
use smartstring::alias::String as SmartString;
use std::{
    fmt::{self, Debug, Display, Formatter},
    hash::Hash,
    str::FromStr,
};

use super::{KnownHeaderName, UnknownHeaderName};

/// The name of a http header. This can be either a
/// [`KnownHeaderName`] or a string representation of an unknown
/// header.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HeaderName<'a>(pub(super) HeaderNameInner<'a>);

#[cfg(feature = "serde")]
impl serde::Serialize for HeaderName<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_ref())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum HeaderNameInner<'a> {
    /// A `KnownHeaderName`
    KnownHeader(KnownHeaderName),
    UnknownHeader(UnknownHeaderName<'a>),
}
use crate::Error;
use HeaderNameInner::{KnownHeader, UnknownHeader};

impl<'a> HeaderName<'a> {
    /// Convert a potentially-borrowed headername to a static
    /// headername _by value_.
    #[must_use]
    pub fn into_owned(self) -> HeaderName<'static> {
        HeaderName(match self.0 {
            KnownHeader(known) => KnownHeader(known),
            UnknownHeader(UnknownHeaderName(smartcow)) => {
                UnknownHeader(UnknownHeaderName(smartcow.into_owned()))
            }
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

impl From<String> for HeaderName<'static> {
    fn from(s: String) -> Self {
        Self(match s.parse::<KnownHeaderName>() {
            Ok(khn) => KnownHeader(khn),
            Err(()) => UnknownHeader(UnknownHeaderName(SmartCow::Owned(s.into()))),
        })
    }
}

impl<'a> From<&'a str> for HeaderName<'a> {
    fn from(s: &'a str) -> Self {
        Self(match s.parse::<KnownHeaderName>() {
            Ok(khn) => KnownHeader(khn),
            Err(_e) => UnknownHeader(UnknownHeaderName(SmartCow::Borrowed(s))),
        })
    }
}

impl FromStr for HeaderName<'static> {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_ascii() {
            Ok(Self(match s.parse::<KnownHeaderName>() {
                Ok(known) => KnownHeader(known),
                Err(()) => UnknownHeader(UnknownHeaderName(SmartCow::Owned(SmartString::from(s)))),
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
