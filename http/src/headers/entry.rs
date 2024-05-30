use super::{HeaderName, HeaderValues, KnownHeaderName, UnknownHeaderName};
use hashbrown::hash_map;
use std::{
    collections::btree_map,
    fmt::{self, Debug, Formatter},
};

/// A view into the storage for a particular header name
#[derive(Debug)]
pub enum Entry<'a> {
    /// A mutable view into the location that header values would be inserted into this
    /// [`Headers`][super::Headers] for the specified `HeaderName`
    Vacant(VacantEntry<'a>),
    /// A mutable view into the header values are stored for the specified `HeaderName`
    Occupied(OccupiedEntry<'a>),
}

/// A view into a vacant entry in particular `Headers` for a given `HeaderName`.
///
/// It is part of the [`Entry`] enum.
pub struct VacantEntry<'a>(pub(super) VacantEntryInner<'a>);

impl<'a> Debug for VacantEntry<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("VacantEntry")
            .field("name", &self.name())
            .finish()
    }
}

pub(super) enum VacantEntryInner<'a> {
    Known(btree_map::VacantEntry<'a, KnownHeaderName, HeaderValues>),
    Unknown(hash_map::VacantEntry<'a, UnknownHeaderName<'static>, HeaderValues>),
}

/// A view into an occupied entry in particular `Headers` for a given `HeaderName`.
///
/// It is part of the [`Entry`] enum.
pub struct OccupiedEntry<'a>(pub(super) OccupiedEntryInner<'a>);

impl<'a> Debug for OccupiedEntry<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("OccupiedEntry")
            .field("name", &self.name())
            .field("values", &self.values())
            .finish()
    }
}

pub(super) enum OccupiedEntryInner<'a> {
    Known(btree_map::OccupiedEntry<'a, KnownHeaderName, HeaderValues>),
    Unknown(hash_map::OccupiedEntry<'a, UnknownHeaderName<'static>, HeaderValues>),
}

impl<'a> Entry<'a> {
    /// retrieve the `HeaderName` for this entry.
    pub fn name(&self) -> HeaderName<'_> {
        match self {
            Entry::Vacant(v) => v.name(),
            Entry::Occupied(o) => o.name(),
        }
    }

    /// Sets the value of the entry, and returns a mutable reference to the inserted value.
    ///
    /// Note that this drops any previous occupied value.
    pub fn insert(self, values: impl Into<HeaderValues>) -> &'a mut HeaderValues {
        match self {
            Entry::Vacant(v) => v.insert(values),
            Entry::Occupied(mut o) => {
                o.insert(values);
                o.into_mut()
            }
        }
    }

    /// Sets the value of the entry if it is vacant, and appends the new header values to the
    /// previous ones if occupied. Returns a mutable reference to the inserted or updated value.
    pub fn append(self, values: impl Into<HeaderValues>) -> &'a mut HeaderValues {
        match self {
            Entry::Vacant(v) => v.insert(values),
            Entry::Occupied(mut o) => {
                o.values_mut().extend(values);
                o.into_mut()
            }
        }
    }

    /// Sets the value if previously vacant. The provided function is only executed if the entry is
    /// vacant.
    pub fn or_insert_with<V: Into<HeaderValues>>(
        self,
        values: impl FnOnce() -> V,
    ) -> &'a mut HeaderValues {
        match self {
            Self::Occupied(entry) => entry.into_mut(),
            Self::Vacant(entry) => entry.insert(values()),
        }
    }

    /// Sets the value if previously vacant. See also [`Entry::or_insert_with`] if constructing the
    /// default value is expensive.
    pub fn or_insert(self, values: impl Into<HeaderValues>) -> &'a mut HeaderValues {
        match self {
            Entry::Vacant(vacant) => vacant.insert(values),
            Entry::Occupied(occupied) => occupied.into_mut(),
        }
    }

    /// Provides in-place mutable access to an occupied entry before any
    /// potential inserts with [`Entry::or_insert`] or [`Entry::or_insert_with`].
    pub fn and_modify(self, f: impl FnOnce(&mut HeaderValues)) -> Self {
        match self {
            Self::Occupied(mut entry) => {
                f(entry.values_mut());
                Self::Occupied(entry)
            }
            Self::Vacant(entry) => Self::Vacant(entry),
        }
    }

    /// Predicate to determine if this is a `VacantEntry`
    pub fn is_vacant(&self) -> bool {
        matches!(self, Self::Vacant(_))
    }

    /// Predicate to determine if this is an `OccupiedEntry`
    pub fn is_occupied(&self) -> bool {
        matches!(self, Self::Occupied(_))
    }

    /// Return the `OccupiedEntry`, if this entry is occupied
    pub fn occupied(self) -> Option<OccupiedEntry<'a>> {
        match self {
            Entry::Vacant(_) => None,
            Entry::Occupied(o) => Some(o),
        }
    }

    /// Return the `VacantEntry`, if this entry is vacant
    pub fn vacant(self) -> Option<VacantEntry<'a>> {
        match self {
            Entry::Vacant(v) => Some(v),
            Entry::Occupied(_) => None,
        }
    }
}

impl<'a> VacantEntry<'a> {
    /// Retrieves the `HeaderName` for this `Entry`.
    pub fn name(&self) -> HeaderName<'_> {
        match &self.0 {
            VacantEntryInner::Known(k) => (*k.key()).into(),
            VacantEntryInner::Unknown(u) => u.key().reborrow().into(),
        }
    }

    /// Replace this `VacantEntry` with a value, returning a mutable reference to that value.
    pub fn insert(self, values: impl Into<HeaderValues>) -> &'a mut HeaderValues {
        match self.0 {
            VacantEntryInner::Known(k) => k.insert(values.into()),
            VacantEntryInner::Unknown(u) => u.insert(values.into()),
        }
    }
}

impl<'a> OccupiedEntry<'a> {
    /// Retrieves the `HeaderName` for this `Entry`
    pub fn name(&self) -> HeaderName<'_> {
        match &self.0 {
            OccupiedEntryInner::Known(known) => (*known.key()).into(),
            OccupiedEntryInner::Unknown(unknown) => unknown.key().reborrow().into(),
        }
    }

    /// Borrows the `HeaderValues` for this `Entry`
    pub fn values(&self) -> &HeaderValues {
        match &self.0 {
            OccupiedEntryInner::Known(known) => known.get(),
            OccupiedEntryInner::Unknown(unknown) => unknown.get(),
        }
    }

    /// Mutate the `HeaderValues` for this `Entry`
    pub fn values_mut(&mut self) -> &mut HeaderValues {
        match &mut self.0 {
            OccupiedEntryInner::Known(known) => known.get_mut(),
            OccupiedEntryInner::Unknown(unknown) => unknown.get_mut(),
        }
    }

    /// Take ownership of the `HeaderName` and `HeaderValues` represented by this entry, removing it
    /// from the `Headers`
    pub fn remove_entry(self) -> (HeaderName<'static>, HeaderValues) {
        match self.0 {
            OccupiedEntryInner::Known(known) => {
                let (n, v) = known.remove_entry();
                (n.into(), v)
            }
            OccupiedEntryInner::Unknown(unknown) => {
                let (n, v) = unknown.remove_entry();
                (n.into(), v)
            }
        }
    }

    /// Take ownership of the `HeaderValues` contained in this entry, removing it from the `Headers`
    pub fn remove(self) -> HeaderValues {
        match self.0 {
            OccupiedEntryInner::Known(known) => known.remove(),
            OccupiedEntryInner::Unknown(unknown) => unknown.remove(),
        }
    }

    /// Converts this `OccupiedEntry` into a mutable reference to the value in the entry with a
    /// lifetime bound to the Headers itself.
    ///
    /// If you need multiple references to the `OccupiedEntry`, see [`OccupiedEntry::values_mut`]
    pub fn into_mut(self) -> &'a mut HeaderValues {
        match self.0 {
            OccupiedEntryInner::Known(k) => k.into_mut(),
            OccupiedEntryInner::Unknown(u) => u.into_mut(),
        }
    }

    /// Sets the value of the entry, and returns the entry's old value.
    pub fn insert(&mut self, values: impl Into<HeaderValues>) -> HeaderValues {
        match &mut self.0 {
            OccupiedEntryInner::Known(k) => k.insert(values.into()),
            OccupiedEntryInner::Unknown(u) => u.insert(values.into()),
        }
    }

    /// Adds additional `HeaderValues` to the existing `HeaderValues` in this entry.
    pub fn append(&mut self, values: impl Into<HeaderValues>) {
        self.values_mut().extend(values);
    }
}
