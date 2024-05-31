use crate::HeaderValue;
use smallvec::{smallvec, SmallVec};
use smartcow::SmartCow;
use std::{
    borrow::Cow,
    fmt::{Debug, Formatter, Result},
    ops::{Deref, DerefMut},
};

/// A header value is a collection of one or more [`HeaderValue`]. It
/// has been optimized for the "one [`HeaderValue`]" case, but can
/// accomodate more than one value.
#[derive(Clone, Eq, PartialEq)]
pub struct HeaderValues(SmallVec<[HeaderValue; 1]>);
impl Deref for HeaderValues {
    type Target = [HeaderValue];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for HeaderValues {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match &**self {
            [one] => one.serialize(serializer),
            several => several.serialize(serializer),
        }
    }
}

impl Default for HeaderValues {
    fn default() -> Self {
        Self(SmallVec::with_capacity(1))
    }
}

impl DerefMut for HeaderValues {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Debug for HeaderValues {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self.one() {
            Some(one) => Debug::fmt(one, f),
            None => f.debug_list().entries(&self.0).finish(),
        }
    }
}

impl PartialEq<[&str]> for HeaderValues {
    fn eq(&self, other: &[&str]) -> bool {
        &**self == other
    }
}

impl IntoIterator for HeaderValues {
    type Item = HeaderValue;

    type IntoIter = smallvec::IntoIter<[HeaderValue; 1]>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a HeaderValues {
    type Item = &'a HeaderValue;

    type IntoIter = std::slice::Iter<'a, HeaderValue>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<I> FromIterator<I> for HeaderValues
where
    I: Into<HeaderValue>,
{
    fn from_iter<T: IntoIterator<Item = I>>(iter: T) -> Self {
        Self(iter.into_iter().map(Into::into).collect())
    }
}

impl HeaderValues {
    /// Builds an empty `HeaderValues`. This is not generally necessary
    /// in application code. Using a `From` implementation is preferable.
    #[must_use]
    pub fn new() -> Self {
        Self(SmallVec::with_capacity(1))
    }

    /// If there is only a single value, returns that header as a
    /// borrowed string slice if it is utf8. If there are more than
    /// one header value, or if the singular header value is not utf8,
    /// `as_str` returns None.
    pub fn as_str(&self) -> Option<&str> {
        self.one().and_then(HeaderValue::as_str)
    }

    pub(crate) fn as_lower(&self) -> Option<SmartCow<'_>> {
        self.one().and_then(HeaderValue::as_lower)
    }

    /// If there is only a single `HeaderValue` inside this
    /// `HeaderValues`, `one` returns a reference to that value. If
    /// there are more than one header value inside this
    /// `HeaderValues`, `one` returns None.
    pub fn one(&self) -> Option<&HeaderValue> {
        if self.len() == 1 {
            self.0.first()
        } else {
            None
        }
    }

    /// Add another header value to this `HeaderValues`.
    pub fn append(&mut self, value: impl Into<HeaderValue>) {
        self.0.push(value.into());
    }

    /// Adds any number of other header values to this `HeaderValues`.
    pub fn extend(&mut self, values: impl Into<HeaderValues>) {
        let values = values.into();
        self.0.extend(values);
    }
}

// impl AsRef<[u8]> for HeaderValues {
//     fn as_ref(&self) -> &[u8] {
//         self.one().as_ref()
//     }
// }

macro_rules! delegate_from_to_header_value {
    ($($t:ty),*) => {
        $(
        impl From<$t> for HeaderValues {
            fn from(value: $t) -> Self {
                HeaderValue::from(value).into()
            }
        }
        )*
    };
}

delegate_from_to_header_value!(
    &'static [u8],
    Vec<u8>,
    String,
    usize,
    u64,
    u16,
    u32,
    i32,
    i64,
    Cow<'static, str>,
    &'static str,
    std::fmt::Arguments<'_>
);

impl From<HeaderValue> for HeaderValues {
    fn from(v: HeaderValue) -> Self {
        Self(smallvec![v])
    }
}

impl<const N: usize, HV> From<[HV; N]> for HeaderValues
where
    HV: Into<HeaderValue>,
{
    fn from(v: [HV; N]) -> Self {
        Self(v.into_iter().map(Into::into).collect())
    }
}

impl<HV> From<Vec<HV>> for HeaderValues
where
    HV: Into<HeaderValue>,
{
    fn from(value: Vec<HV>) -> Self {
        Self(value.into_iter().map(Into::into).collect())
    }
}

impl<'a, HV> From<&'a [HV]> for HeaderValues
where
    &'a HV: Into<HeaderValue>,
{
    fn from(value: &'a [HV]) -> Self {
        Self(value.iter().map(Into::into).collect())
    }
}

impl PartialEq<str> for HeaderValues {
    fn eq(&self, other: &str) -> bool {
        self.as_str().is_some_and(|v| v == other)
    }
}

impl PartialEq<&str> for HeaderValues {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}
