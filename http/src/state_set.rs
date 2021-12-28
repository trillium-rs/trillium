// Originally from https://github.com/http-rs/http-types/blob/main/src/extensions.rs
//
// Implementation is based on
// - https://github.com/hyperium/http/blob/master/src/extensions.rs
// - https://github.com/kardeiz/type-map/blob/master/src/lib.rs
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    fmt,
    hash::{BuildHasherDefault, Hasher},
};

/// Store and retrieve values by
/// [`TypeId`](https://doc.rust-lang.org/std/any/struct.TypeId.html). This
/// allows storing arbitrary data that implements `Sync + Send +
/// 'static`.
#[derive(Default)]
pub struct StateSet(HashMap<TypeId, Box<dyn Any + Send + Sync>, BuildHasherDefault<IdHasher>>);

impl StateSet {
    /// Create an empty `StateSet`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a value into this `StateSet`.
    ///
    /// If a value of this type already exists, it will be returned.
    pub fn insert<T: Send + Sync + 'static>(&mut self, val: T) -> Option<T> {
        self.0
            .insert(TypeId::of::<T>(), Box::new(val))
            .and_then(|boxed| (boxed as Box<dyn Any>).downcast().ok().map(|boxed| *boxed))
    }

    /// Check if container contains value for type
    pub fn contains<T: 'static>(&self) -> bool {
        self.0.get(&TypeId::of::<T>()).is_some()
    }

    /// Get a reference to a value previously inserted on this `StateSet`.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.0
            .get(&TypeId::of::<T>())
            .and_then(|boxed| (&**boxed as &(dyn Any)).downcast_ref())
    }

    /// Get a mutable reference to a value previously inserted on this `StateSet`.
    pub fn get_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.0
            .get_mut(&TypeId::of::<T>())
            .and_then(|boxed| (&mut **boxed as &mut (dyn Any)).downcast_mut())
    }

    /// Remove a value from this `StateSet`.
    ///
    /// If a value of this type exists, it will be returned.
    pub fn take<T: 'static>(&mut self) -> Option<T> {
        self.0
            .remove(&TypeId::of::<T>())
            .and_then(|boxed| (boxed as Box<dyn Any>).downcast().ok().map(|boxed| *boxed))
    }

    /// Gets a value from this StateSet or populates it with the provided default.
    pub fn get_or_insert<T: Send + Sync + 'static>(&mut self, default: T) -> &mut T {
        self.0
            .entry(TypeId::of::<T>())
            .or_insert(Box::new(default))
            .downcast_mut()
            .unwrap()
    }

    /// Gets a value from this StateSet or populates it with the provided default function.
    pub fn get_or_insert_with<F, T>(&mut self, default: F) -> &mut T
    where
        F: FnOnce() -> T,
        T: Send + Sync + 'static,
    {
        self.0
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(default()))
            .downcast_mut()
            .unwrap()
    }
}

impl fmt::Debug for StateSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateSet").finish()
    }
}

// With TypeIds as keys, there's no need to hash them. So we simply use an identy hasher.
#[derive(Default)]
struct IdHasher(u64);

impl Hasher for IdHasher {
    fn write(&mut self, _: &[u8]) {
        unreachable!("TypeId calls write_u64");
    }

    #[inline]
    fn write_u64(&mut self, id: u64) {
        self.0 = id;
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_extensions() {
        #[derive(Debug, PartialEq)]
        struct MyType(i32);

        let mut map = StateSet::new();

        map.insert(5i32);
        map.insert(MyType(10));

        assert_eq!(map.get(), Some(&5i32));
        assert_eq!(map.get_mut(), Some(&mut 5i32));

        assert_eq!(map.take::<i32>(), Some(5i32));
        assert!(map.get::<i32>().is_none());

        assert_eq!(map.get::<bool>(), None);
        assert_eq!(map.get(), Some(&MyType(10)));
    }
}
