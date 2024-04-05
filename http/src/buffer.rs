use std::{
    fmt::{Debug, Formatter},
    ops::{Deref, DerefMut},
    str,
};

#[derive(Default)]
#[doc(hidden)]
pub struct Buffer(usize, Vec<u8>);

impl Debug for Buffer {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let slice = self.as_slice();
        if let Ok(str) = str::from_utf8(slice) {
            str.fmt(f)
        } else {
            slice.fmt(f)
        }
    }
}
impl From<Buffer> for Vec<u8> {
    fn from(Buffer(offset, mut vec): Buffer) -> Self {
        vec.copy_within(offset.., 0);
        vec.truncate(vec.len() - offset);
        vec
    }
}
impl From<Vec<u8>> for Buffer {
    fn from(value: Vec<u8>) -> Self {
        Self(0, value)
    }
}
impl Deref for Buffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}
impl DerefMut for Buffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}
#[doc(hidden)]
impl Buffer {
    pub fn truncate(&mut self, n: usize) {
        if n == 0 {
            self.0 = 0;
            self.1.truncate(0);
        } else {
            self.1.truncate(self.0 + n);
        }
    }

    pub fn extend_from_slice(&mut self, slice: &[u8]) {
        self.1.extend_from_slice(slice);
    }

    pub fn ignore_front(&mut self, n: usize) {
        self.0 += n;
        if self.0 >= self.1.len() {
            self.1.truncate(0);
            self.0 = 0;
        }
    }

    pub fn len(&self) -> usize {
        self.1.len() - self.0
    }

    pub fn is_empty(&self) -> bool {
        self.1.len() == self.0
    }

    pub fn fill_capacity(&mut self) {
        self.1.resize(self.1.capacity(), 0);
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self(0, Vec::with_capacity(capacity))
    }

    pub fn expand(&mut self) {
        if self.1.len() == self.1.capacity() {
            self.1.reserve(32);
        }
        self.fill_capacity();
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.1[self.0..]
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.1[self.0..]
    }
}
