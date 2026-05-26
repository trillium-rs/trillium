use std::path::{Path, PathBuf};

/// A directory that [`StaticFileHandler`][crate::StaticFileHandler] resolved
/// from the request path but did not serve a file from — either because no
/// index file is configured or because the configured index was absent.
///
/// When this happens, the handler leaves the conn unhalted and stores the
/// resolved directory in conn state rather than serving it, so a subsequent
/// handler can enumerate the directory and render a listing. The contained
/// path has already passed the handler's traversal-containment check, so it is
/// guaranteed to be within the served root.
///
/// Retrieve it with
/// [`StaticConnExt::resolved_directory`][crate::StaticConnExt::resolved_directory].
#[derive(Debug, Clone)]
pub struct ResolvedDirectory(PathBuf);

impl ResolvedDirectory {
    pub(crate) const fn new(path: PathBuf) -> Self {
        Self(path)
    }

    /// The resolved filesystem path of the directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}
