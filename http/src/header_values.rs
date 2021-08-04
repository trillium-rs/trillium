use crate::HeaderValue;
use cervine::Cow;
use smallvec::{smallvec, SmallVec};
use smartstring::alias::String as SmartString;
use std::{
    fmt::{Debug, Display},
    iter::FromIterator,
    ops::{Deref, DerefMut},
};

#[derive(Clone, Eq, PartialEq)]
pub struct HeaderValues(SmallVec<[HeaderValue; 1]>);
impl Deref for HeaderValues {
    type Target = [HeaderValue];

    fn deref(&self) -> &Self::Target {
        &self.0
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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.len() == 1 {
            Debug::fmt(self.one(), f)
        } else {
            f.debug_list().entries(&self.0).finish()
        }
    }
}

impl IntoIterator for HeaderValues {
    type Item = HeaderValue;

    type IntoIter = smallvec::IntoIter<[HeaderValue; 1]>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
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
    pub fn new() -> Self {
        Self(SmallVec::with_capacity(1))
    }

    pub fn as_str(&self) -> Option<&str> {
        self.one().as_str()
    }

    pub fn as_lower(&self) -> Option<Cow<'_, SmartString, str>> {
        self.one().as_lower()
    }

    pub fn one(&self) -> &HeaderValue {
        self.0.last().unwrap()
    }

    pub fn append(&mut self, value: impl Into<HeaderValue>) {
        self.0.push(value.into());
    }

    pub fn extend(&mut self, values: impl Into<HeaderValues>) {
        let values = values.into();
        self.0.extend(values);
    }
}

impl AsRef<[u8]> for HeaderValues {
    fn as_ref(&self) -> &[u8] {
        self.one().as_ref()
    }
}

impl Display for HeaderValues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self.one(), f)
    }
}

impl From<Vec<u8>> for HeaderValues {
    fn from(v: Vec<u8>) -> Self {
        Self(smallvec![v.into()])
    }
}

impl From<String> for HeaderValues {
    fn from(s: String) -> Self {
        Self(smallvec![s.into()])
    }
}

impl From<&'static str> for HeaderValues {
    fn from(s: &'static str) -> Self {
        Self(smallvec![s.into()])
    }
}

impl From<HeaderValue> for HeaderValues {
    fn from(v: HeaderValue) -> Self {
        Self(smallvec![v])
    }
}

impl<HV> From<Vec<HV>> for HeaderValues
where
    HV: Into<HeaderValue>,
{
    fn from(v: Vec<HV>) -> Self {
        Self(v.into_iter().map(|hv| hv.into()).collect())
    }
}
