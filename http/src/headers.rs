use crate::{HeaderName, HeaderValue, HeaderValues, KnownHeaderName};
use hashbrown::HashMap;
use smartcow::SmartCow;
use smartstring::alias::String;
use std::{
    fmt::Debug,
    hash::{BuildHasherDefault, Hasher},
    iter::FromIterator,
};

#[derive(Debug, Clone)]
pub struct Headers {
    known: HashMap<KnownHeaderName, HeaderValues, BuildHasherDefault<DirectHasher>>,
    unknown: HashMap<SmartCow<'static>, HeaderValues>,
}

#[derive(Default)]
struct DirectHasher(u8);

impl Hasher for DirectHasher {
    fn write(&mut self, _: &[u8]) {
        unreachable!("KnownHeaderName calls write_u64");
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.0 = i;
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0 as u64
    }
}

impl Default for Headers {
    fn default() -> Self {
        Self::with_capacity(15)
    }
}

impl Headers {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            known: HashMap::with_capacity_and_hasher(capacity, BuildHasherDefault::default()),
            unknown: HashMap::with_capacity(0),
        }
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn reserve(&mut self, additional: usize) {
        self.known.reserve(additional);
    }

    pub fn iter<'a>(&'a self) -> impl Iterator<Item = (HeaderName<'a>, &'a HeaderValues)> {
        self.known
            .iter()
            .map(|(k, v)| (HeaderName::KnownHeader(*k), v))
            .chain(
                self.unknown
                    .iter()
                    .map(|(k, v)| (HeaderName::UnknownHeader(SmartCow::Borrowed(&*k)), v)),
            )
    }

    pub fn into_iter(self) -> impl Iterator<Item = (HeaderName<'static>, HeaderValues)> {
        let Self { known, unknown } = self;

        known
            .into_iter()
            .map(|(k, v)| (HeaderName::KnownHeader(k), v))
            .chain(
                unknown
                    .into_iter()
                    .map(|(k, v)| (HeaderName::UnknownHeader(k), v)),
            )
    }

    pub fn append(&mut self, name: impl Into<HeaderName<'static>>, value: impl Into<HeaderValues>) {
        match name.into() {
            HeaderName::KnownHeader(known) => {
                self.known
                    .entry(known)
                    .or_insert_with(HeaderValues::new)
                    .extend(value);
            }
            HeaderName::UnknownHeader(unknown) => {
                self.unknown
                    .entry(unknown)
                    .or_insert_with(HeaderValues::new)
                    .extend(value);
            }
        }
    }

    pub fn insert(&mut self, name: impl Into<HeaderName<'static>>, value: impl Into<HeaderValues>) {
        match name.into() {
            HeaderName::KnownHeader(known) => {
                self.known.insert(known, value.into());
            }

            HeaderName::UnknownHeader(unknown) => {
                self.unknown.insert(unknown, value.into());
            }
        }
    }

    pub fn try_insert(
        &mut self,
        name: impl Into<HeaderName<'static>>,
        value: impl Into<HeaderValues>,
    ) {
        match name.into() {
            HeaderName::KnownHeader(known) => {
                self.known.entry(known).or_insert(value.into());
            }
            HeaderName::UnknownHeader(unknown) => {
                self.unknown.entry(unknown).or_insert(value.into());
            }
        }
    }

    pub fn get_str<'a>(&'a self, name: impl Into<HeaderName<'a>>) -> Option<&str> {
        self.get_values(name).and_then(HeaderValues::as_str)
    }

    pub fn get_lower<'a>(&'a self, name: impl Into<HeaderName<'a>>) -> Option<SmartCow<'_>> {
        self.get_values(name).and_then(HeaderValues::as_lower)
    }

    pub fn get<'a>(&'a self, name: impl Into<HeaderName<'a>>) -> Option<&HeaderValue> {
        self.get_values(name).map(HeaderValues::one)
    }

    pub fn remove(&mut self, name: impl Into<HeaderName<'static>>) -> Option<HeaderValues> {
        match name.into() {
            HeaderName::KnownHeader(known) => self.known.remove(&known),
            HeaderName::UnknownHeader(unknown) => self.unknown.remove(&unknown),
        }
    }

    pub fn get_values<'a>(&'a self, name: impl Into<HeaderName<'a>>) -> Option<&HeaderValues> {
        match name.into() {
            HeaderName::KnownHeader(known) => self.known.get(&known),
            HeaderName::UnknownHeader(unknown) => self.unknown.get(&unknown),
        }
    }

    pub fn has_header<'a>(&'a self, name: impl Into<HeaderName<'a>>) -> bool {
        match name.into() {
            HeaderName::KnownHeader(known) => self.known.contains_key(&known),
            HeaderName::UnknownHeader(unknown) => self.unknown.contains_key(&unknown),
        }
    }

    pub fn eq_ignore_ascii_case<'a>(
        &'a self,
        name: impl Into<HeaderName<'a>>,
        needle: &str,
    ) -> bool {
        self.get(name).map(|v| v == needle).unwrap_or_default()
    }

    pub fn contains_ignore_ascii_case<'a>(
        &'a self,
        name: impl Into<HeaderName<'a>>,
        needle: &str,
    ) -> bool {
        self.get(name)
            .and_then(HeaderValue::as_str)
            .map(|h| {
                h.to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase())
            })
            .unwrap_or_default()
    }
}

impl<HN, HV> Extend<(HN, HV)> for Headers
where
    HN: Into<HeaderName<'static>>,
    HV: Into<HeaderValues>,
{
    fn extend<T: IntoIterator<Item = (HN, HV)>>(&mut self, iter: T) {
        let iter = iter.into_iter();
        match iter.size_hint() {
            (additional, _) if additional > 0 => self.known.reserve(additional),
            _ => {}
        };

        for (name, values) in iter {
            self.append(name, values);
        }
    }
}

impl<HN, HV> FromIterator<(HN, HV)> for Headers
where
    HN: Into<HeaderName<'static>>,
    HV: Into<HeaderValues>,
{
    fn from_iter<T: IntoIterator<Item = (HN, HV)>>(iter: T) -> Self {
        let iter = iter.into_iter();
        let mut headers = match iter.size_hint() {
            (0, _) => Self::new(),
            (n, _) => Self::with_capacity(n),
        };

        for (name, values) in iter {
            headers.append(name, values);
        }

        headers
    }
}
