use std::{
    fmt::{self, Debug, Formatter},
    path::Path,
};

/// A file with its contents stored in a `&'static [u8]`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct File {
    path: &'static str,
    contents: &'static [u8],
    metadata: Option<crate::Metadata>,
}

impl File {
    /// Create a new [`File`].
    pub const fn new(path: &'static str, contents: &'static [u8]) -> Self {
        File {
            path,
            contents,
            metadata: None,
        }
    }

    /// The full path for this [`File`], relative to the directory passed to
    /// [`crate::include_dir!()`].
    pub fn path(&self) -> &'static Path {
        Path::new(self.path)
    }

    /// The file's raw contents.
    pub fn contents(&self) -> &'static [u8] {
        self.contents
    }

    /// The file's contents interpreted as a string.
    pub fn contents_utf8(&self) -> Option<&'static str> {
        std::str::from_utf8(self.contents()).ok()
    }

    /// Set the [`Metadata`] associated with a [`File`].
    pub const fn with_metadata(self, metadata: crate::Metadata) -> Self {
        let File { path, contents, .. } = self;

        File {
            path,
            contents,
            metadata: Some(metadata),
        }
    }

    /// Get the [`File`]'s [`Metadata`], if available.
    pub fn metadata(&self) -> Option<&crate::Metadata> {
        self.metadata.as_ref()
    }
}

impl Debug for File {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("File")
            .field("path", &self.path)
            .field("contents", &format_args!("<{} bytes>", self.contents.len()))
            .field("metadata", &self.metadata)
            .finish()
    }
}
