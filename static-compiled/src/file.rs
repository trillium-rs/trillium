use crate::Encoding;
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
    encodings: &'static [(Encoding, &'static [u8])],
}

impl File {
    /// Create a new [`File`].
    pub const fn new(path: &'static str, contents: &'static [u8]) -> Self {
        File {
            path,
            contents,
            metadata: None,
            encodings: &[],
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

    /// Set the [`Metadata`](crate::Metadata) associated with a [`File`].
    pub const fn with_metadata(self, metadata: crate::Metadata) -> Self {
        let File {
            path,
            contents,
            encodings,
            ..
        } = self;

        File {
            path,
            contents,
            metadata: Some(metadata),
            encodings,
        }
    }

    /// Get the [`File`]'s [`Metadata`](crate::Metadata), if available.
    pub fn metadata(&self) -> Option<&crate::Metadata> {
        self.metadata.as_ref()
    }

    /// Attach precompressed variants. Used by the `static_compiled!` macro
    /// when compression is requested; not generally called directly.
    ///
    /// Variants are expected to be sorted smallest-first so that
    /// [`pick_encoding`](Self::pick_encoding) returns the smallest variant
    /// the client accepts.
    pub const fn with_encodings(self, encodings: &'static [(Encoding, &'static [u8])]) -> Self {
        let File {
            path,
            contents,
            metadata,
            ..
        } = self;

        File {
            path,
            contents,
            metadata,
            encodings,
        }
    }

    /// All precompressed variants attached to this file, in server-preference
    /// order (smallest-first).
    pub const fn encodings(&self) -> &'static [(Encoding, &'static [u8])] {
        self.encodings
    }

    /// Returns the first precompressed variant whose encoding is permitted by
    /// the supplied `Accept-Encoding` header value, or `None` if no variants
    /// are attached, no header is supplied, or none are accepted.
    pub fn pick_encoding(&self, accept: Option<&str>) -> Option<(Encoding, &'static [u8])> {
        let accept = accept?;
        self.encodings
            .iter()
            .copied()
            .find(|(encoding, _)| crate::encoding::accept_encoding_allows(accept, encoding.token()))
    }
}

impl Debug for File {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("File")
            .field("path", &self.path)
            .field("contents", &format_args!("<{} bytes>", self.contents.len()))
            .field("metadata", &self.metadata)
            .field(
                "encodings",
                &format_args!("<{} variants>", self.encodings.len()),
            )
            .finish()
    }
}
