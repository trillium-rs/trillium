use crate::{header_name::HeaderNameInner, HeaderName, HeaderValue, HeaderValues, KnownHeaderName};
use hashbrown::{hash_map::Entry, HashMap};
use smartcow::SmartCow;
use std::{
    fmt::Debug,
    hash::{BuildHasherDefault, Hasher},
    iter::FromIterator,
};

/// Trillium's header map type
#[derive(Debug, Clone)]
pub struct Headers {
    known: HashMap<KnownHeaderName, HeaderValues, BuildHasherDefault<DirectHasher>>,
    unknown: HashMap<SmartCow<'static>, HeaderValues>,
}

impl Default for Headers {
    fn default() -> Self {
        Self::with_capacity(15)
    }
}

impl Headers {
    /// Construct a new Headers, expecting to see at least this many known headers.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            known: HashMap::with_capacity_and_hasher(capacity, BuildHasherDefault::default()),
            unknown: HashMap::with_capacity(0),
        }
    }

    /// Construct a new headers with a default capacity of 15 known headers
    pub fn new() -> Self {
        Self::default()
    }

    /// Extend the capacity of the known headers map by this many
    pub fn reserve(&mut self, additional: usize) {
        self.known.reserve(additional);
    }

    /// Return an iterator over borrowed header names and header
    /// values. First yields the known headers and then the unknown
    /// headers, if any.
    pub fn iter(&self) -> impl Iterator<Item = (HeaderName<'_>, &'_ HeaderValues)> {
        self.known
            .iter()
            .map(|(k, v)| (HeaderName(HeaderNameInner::KnownHeader(*k)), v))
            .chain(self.unknown.iter().map(|(k, v)| {
                (
                    HeaderName(HeaderNameInner::UnknownHeader(SmartCow::Borrowed(&*k))),
                    v,
                )
            }))
    }

    /// add the header value or header values into this header map. If
    /// there is already a header with the same name, the new values
    /// will be added to the existing ones. To replace any existing
    /// values, use [`Headers::insert`]
    pub fn append(&mut self, name: impl Into<HeaderName<'static>>, value: impl Into<HeaderValues>) {
        let value = value.into();
        match name.into().0 {
            HeaderNameInner::KnownHeader(known) => match self.known.entry(known) {
                Entry::Occupied(mut o) => {
                    o.get_mut().extend(value);
                }
                Entry::Vacant(v) => {
                    v.insert(value);
                }
            },

            HeaderNameInner::UnknownHeader(unknown) => match self.unknown.entry(unknown) {
                Entry::Occupied(mut o) => {
                    o.get_mut().extend(value);
                }
                Entry::Vacant(v) => {
                    v.insert(value);
                }
            },
        }
    }

    /// Add a header value or header values into this header map. If a
    /// header already exists with the same name, it will be
    /// replaced. To combine, see [`Headers::append`]
    pub fn insert(&mut self, name: impl Into<HeaderName<'static>>, value: impl Into<HeaderValues>) {
        let value = value.into();
        match name.into().0 {
            HeaderNameInner::KnownHeader(known) => {
                self.known.insert(known, value);
            }

            HeaderNameInner::UnknownHeader(unknown) => {
                self.unknown.insert(unknown, value);
            }
        }
    }

    /// Add a header value or header values into this header map if
    /// and only if there is not already a header with the same name.
    pub fn try_insert(
        &mut self,
        name: impl Into<HeaderName<'static>>,
        value: impl Into<HeaderValues>,
    ) {
        let value = value.into();
        match name.into().0 {
            HeaderNameInner::KnownHeader(known) => {
                self.known.entry(known).or_insert(value);
            }

            HeaderNameInner::UnknownHeader(unknown) => {
                self.unknown.entry(unknown).or_insert(value);
            }
        }
    }

    /// Retrieves a &str header value if there is at least one header
    /// in the map with this name. If there are several headers with
    /// the same name, this follows the behavior defined at
    /// [`HeaderValues::one`]. Returns None if there is no header with
    /// the provided header name.
    pub fn get_str<'a>(&'a self, name: impl Into<HeaderName<'a>>) -> Option<&str> {
        self.get_values(name).and_then(HeaderValues::as_str)
    }

    pub(crate) fn get_lower<'a>(&'a self, name: impl Into<HeaderName<'a>>) -> Option<SmartCow<'_>> {
        self.get_values(name).and_then(HeaderValues::as_lower)
    }

    /// Retrieves a singular header value from this header map. If
    /// there are several headers with the same name, this follows the
    /// behavior defined at [`HeaderValues::one`]. Returns None if there is no header with the provided header name
    pub fn get<'a>(&'a self, name: impl Into<HeaderName<'a>>) -> Option<&HeaderValue> {
        self.get_values(name).and_then(HeaderValues::one)
    }

    /// Takes all headers with the provided header name out of this
    /// header map and returns them. Returns None if the header did
    /// not have an entry in this map.
    pub fn remove(&mut self, name: impl Into<HeaderName<'static>>) -> Option<HeaderValues> {
        match name.into().0 {
            HeaderNameInner::KnownHeader(known) => self.known.remove(&known),
            HeaderNameInner::UnknownHeader(unknown) => self.unknown.remove(&unknown),
        }
    }

    /// Retrieves a reference to all header values with the provided
    /// header name. If you expect there to be only one value, use
    /// [`Headers::get`].
    pub fn get_values<'a>(&'a self, name: impl Into<HeaderName<'a>>) -> Option<&HeaderValues> {
        match name.into().0 {
            HeaderNameInner::KnownHeader(known) => self.known.get(&known),
            HeaderNameInner::UnknownHeader(unknown) => self.unknown.get(&unknown),
        }
    }

    /// Predicate function to check whether this header map contains
    /// the provided header name. If you are using this to
    /// conditionally insert a value, consider using
    /// [`Headers::try_insert`] instead.
    pub fn has_header<'a>(&'a self, name: impl Into<HeaderName<'a>>) -> bool {
        match name.into().0 {
            HeaderNameInner::KnownHeader(known) => self.known.contains_key(&known),
            HeaderNameInner::UnknownHeader(unknown) => self.unknown.contains_key(&unknown),
        }
    }

    /// Convenience function to check whether the value contained in
    /// this header map for the provided name is
    /// ascii-case-insensitively equal to the provided comparison
    /// &str. Returns false if there is no value for the name
    pub fn eq_ignore_ascii_case<'a>(
        &'a self,
        name: impl Into<HeaderName<'a>>,
        needle: &str,
    ) -> bool {
        self.get_str(name).map(|v| v == needle).unwrap_or_default()
    }

    /// Convenience function to check whether the value contained in
    /// this header map for the provided name. Prefer testing against
    /// a lower case string, as the implementation currently has to allocate if .
    pub fn contains_ignore_ascii_case<'a>(
        &'a self,
        name: impl Into<HeaderName<'a>>,
        needle: &str,
    ) -> bool {
        self.get_str(name)
            .map(|h| {
                let needle = if needle.chars().all(|c| c.is_ascii_lowercase()) {
                    SmartCow::Borrowed(needle)
                } else {
                    SmartCow::Owned(needle.chars().map(|c| c.to_ascii_lowercase()).collect())
                };

                if h.chars().all(|c| c.is_ascii_lowercase()) {
                    h.contains(&*needle)
                } else {
                    h.to_ascii_lowercase().contains(&*needle)
                }
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
        u64::from(self.0)
    }
}

impl IntoIterator for Headers {
    type Item = (HeaderName<'static>, HeaderValues);

    type IntoIter = Box<dyn Iterator<Item = Self::Item>>;

    fn into_iter(self) -> Self::IntoIter {
        let Self { known, unknown } = self;

        Box::new(
            known
                .into_iter()
                .map(|(k, v)| (HeaderName(HeaderNameInner::KnownHeader(k)), v))
                .chain(
                    unknown
                        .into_iter()
                        .map(|(k, v)| (HeaderName(HeaderNameInner::UnknownHeader(k)), v)),
                ),
        )
    }
}
