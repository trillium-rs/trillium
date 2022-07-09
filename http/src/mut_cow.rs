use std::{
    fmt::{self, Debug, Formatter},
    ops::{Deref, DerefMut},
};

#[doc(hidden)]
pub enum MutCow<'a, T> {
    Owned(T),
    Borrowed(&'a mut T),
}

impl<'a, T: Debug> Debug for MutCow<'a, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(&**self, f)
    }
}

impl<'a, T> MutCow<'a, T> {
    pub fn is_owned(&self) -> bool {
        matches!(self, MutCow::Owned(_))
    }

    pub(crate) fn unwrap_owned(self) -> T {
        match self {
            MutCow::Owned(t) => t,
            MutCow::Borrowed(_) => panic!("attempted to unwrap a borrow"),
        }
    }
}

impl<'a, T> Deref for MutCow<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            MutCow::Owned(t) => t,
            MutCow::Borrowed(t) => t,
        }
    }
}

impl<'a, T> DerefMut for MutCow<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            MutCow::Owned(t) => t,
            MutCow::Borrowed(t) => t,
        }
    }
}

impl<T> From<T> for MutCow<'static, T> {
    fn from(t: T) -> Self {
        Self::Owned(t)
    }
}

impl<'a, T> From<&'a mut T> for MutCow<'a, T> {
    fn from(t: &'a mut T) -> Self {
        Self::Borrowed(t)
    }
}
