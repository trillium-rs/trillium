use crate::{Dir, File};
use std::path::Path;

/// A directory entry, roughly analogous to [`std::fs::DirEntry`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DirEntry {
    /// A directory.
    Dir(Dir),
    /// A file.
    File(File),
}

impl DirEntry {
    /// The [`DirEntry`]'s full path.
    pub fn path(&self) -> &'static Path {
        match self {
            DirEntry::Dir(d) => d.path(),
            DirEntry::File(f) => f.path(),
        }
    }

    /// Try to get this as a [`Dir`], if it is one.
    pub fn as_dir(&self) -> Option<&Dir> {
        match self {
            DirEntry::Dir(d) => Some(d),
            DirEntry::File(_) => None,
        }
    }

    /// Try to get this as a [`File`], if it is one.
    pub fn as_file(&self) -> Option<&File> {
        match self {
            DirEntry::File(f) => Some(f),
            DirEntry::Dir(_) => None,
        }
    }

    /// Get this item's sub-items, if it has any.
    pub fn children(&self) -> &'static [DirEntry] {
        match self {
            DirEntry::Dir(d) => d.entries(),
            DirEntry::File(_) => &[],
        }
    }

    /// returns true if this entry is a file
    pub fn is_file(&self) -> bool {
        matches!(self, Self::File(_))
    }

    /// returns true if this entry is a dir
    pub fn is_dir(&self) -> bool {
        matches!(self, Self::Dir(_))
    }
}
