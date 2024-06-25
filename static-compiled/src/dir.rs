use crate::{file::File, DirEntry};
use std::{fs, path::Path};

/// A directory.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dir {
    path: &'static str,
    entries: &'static [DirEntry],
}

impl Dir {
    /// Create a new [`Dir`].
    pub const fn new(path: &'static str, entries: &'static [DirEntry]) -> Self {
        Dir { path, entries }
    }

    /// The full path for this [`Dir`], relative to the directory passed to
    /// [`crate::include_dir!()`].
    pub fn path(&self) -> &'static Path {
        Path::new(self.path)
    }

    /// The entries within this [`Dir`].
    pub const fn entries(&self) -> &'static [DirEntry] {
        self.entries
    }

    /// Get a list of the files in this directory.
    pub fn files(&self) -> impl Iterator<Item = &'static File> + 'static {
        self.entries().iter().filter_map(DirEntry::as_file)
    }

    /// Get a list of the sub-directories inside this directory.
    pub fn dirs(&self) -> impl Iterator<Item = &'static Dir> + 'static {
        self.entries().iter().filter_map(DirEntry::as_dir)
    }

    /// Recursively search for a [`DirEntry`] with a particular path.
    pub fn get_entry<S: AsRef<Path>>(&self, path: S) -> Option<&'static DirEntry> {
        let path = path.as_ref();

        for entry in self.entries() {
            if entry.path() == path {
                return Some(entry);
            }

            if let DirEntry::Dir(d) = entry {
                if let Some(nested) = d.get_entry(path) {
                    return Some(nested);
                }
            }
        }

        None
    }

    /// Look up a file by name.
    pub fn get_file<S: AsRef<Path>>(&self, path: S) -> Option<&'static File> {
        self.get_entry(path).and_then(DirEntry::as_file)
    }

    /// Look up a dir by name.
    pub fn get_dir<S: AsRef<Path>>(&self, path: S) -> Option<&'static Dir> {
        self.get_entry(path).and_then(DirEntry::as_dir)
    }

    /// Does this directory contain `path`?
    pub fn contains<S: AsRef<Path>>(&self, path: S) -> bool {
        self.get_entry(path).is_some()
    }

    /// Create directories and extract all files to real filesystem.
    /// Creates parent directories of `path` if they do not already exist.
    /// Fails if some files already exist.
    /// In case of error, partially extracted directory may remain on the filesystem.
    pub fn extract<S: AsRef<Path>>(&self, base_path: S) -> std::io::Result<()> {
        let base_path = base_path.as_ref();

        for entry in self.entries() {
            let path = base_path.join(entry.path());

            match entry {
                DirEntry::Dir(d) => {
                    fs::create_dir_all(&path)?;
                    d.extract(base_path)?;
                }
                DirEntry::File(f) => {
                    fs::write(path, f.contents())?;
                }
            }
        }

        Ok(())
    }
}
