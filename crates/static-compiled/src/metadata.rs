use std::time::{Duration, SystemTime};

/// Basic metadata for a file.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Metadata {
    accessed: Duration,
    created: Duration,
    modified: Duration,
}

impl Metadata {
    /// Create a new [`Metadata`] using Durations since the
    /// [`SystemTime::UNIX_EPOCH`].
    pub const fn new(accessed: Duration, created: Duration, modified: Duration) -> Self {
        Metadata {
            accessed,
            created,
            modified,
        }
    }

    /// Create a new Metadata from the number of seconds since the epoch
    pub const fn from_secs(accessed: u64, created: u64, modified: u64) -> Self {
        Self::new(
            Duration::from_secs(accessed),
            Duration::from_secs(created),
            Duration::from_secs(modified),
        )
    }

    /// Get the time this file was last accessed.
    ///
    /// See also: [`std::fs::Metadata::accessed()`].
    pub fn accessed(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + self.accessed
    }

    /// Get the time this file was created.
    ///
    /// See also: [`std::fs::Metadata::created()`].
    pub fn created(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + self.created
    }

    /// Get the time this file was last modified.
    ///
    /// See also: [`std::fs::Metadata::modified()`].
    pub fn modified(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + self.modified
    }
}
