//! Filesystem capabilities and implementations for Graphcal.
//!
//! This crate owns the [`FileSystemReader`] boundary and provides concrete
//! implementations:
//! - [`RealFileSystem`] — delegates to `std::fs`
//! - [`InMemoryFileSystem`] — for tests and WASM
//! - [`OverlayFileSystem`] — layers in-memory editor buffers over a base reader
//!
//! Reads always carry an explicit byte limit and cancellation signal. This
//! keeps callers from accidentally allocating an unbounded file before they
//! can enforce a policy.

mod in_memory_fs;
mod overlay_fs;
mod real_fs;
mod source_tree;

pub use in_memory_fs::InMemoryFileSystem;
pub use overlay_fs::OverlayFileSystem;
pub use real_fs::RealFileSystem;
pub use source_tree::{
    SourceTreeHash, SourceTreeHashError, SourceTreeHashLimits, hash_source_tree,
};

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Maximum number of bytes one filesystem read may return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteLimit(u64);

impl ByteLimit {
    /// Construct a limit. Zero is a valid deny-all policy.
    #[must_use]
    pub const fn new(bytes: u64) -> Self {
        Self(bytes)
    }

    /// Maximum accepted byte count.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ByteLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Maximum number of entries one directory listing may return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EntryLimit(u64);

impl EntryLimit {
    /// Construct a limit. Zero is a valid deny-all policy.
    #[must_use]
    pub const fn new(entries: u64) -> Self {
        Self(entries)
    }

    /// Maximum accepted entry count.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for EntryLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Cooperative cancellation observed while an I/O implementation reads in
/// bounded chunks.
pub trait CancellationSignal {
    /// Whether the caller no longer needs the operation's result.
    fn is_cancelled(&self) -> bool;
}

impl<F> CancellationSignal for F
where
    F: Fn() -> bool,
{
    fn is_cancelled(&self) -> bool {
        self()
    }
}

/// Cancellation signal for non-interactive operations.
#[derive(Debug, Clone, Copy, Default)]
pub struct NeverCancel;

impl CancellationSignal for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// Failure of a bounded filesystem read.
#[derive(Debug, thiserror::Error)]
pub enum FileSystemReadError {
    /// The underlying filesystem operation failed.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The file was larger than the caller's explicit bound.
    #[error("file exceeds the read limit of {limit} bytes")]
    ByteLimitExceeded {
        /// Maximum accepted byte count.
        limit: ByteLimit,
    },
    /// The directory had more children than the caller allowed.
    #[error("directory exceeds the listing limit of {limit} entries")]
    EntryLimitExceeded {
        /// Maximum accepted child count.
        limit: EntryLimit,
    },
    /// The caller cancelled the read.
    #[error("filesystem read cancelled")]
    Cancelled,
}

impl FileSystemReadError {
    /// Underlying [`io::ErrorKind`] when this is an ordinary I/O failure.
    #[must_use]
    pub fn io_kind(&self) -> Option<io::ErrorKind> {
        match self {
            Self::Io(error) => Some(error.kind()),
            Self::ByteLimitExceeded { .. } | Self::EntryLimitExceeded { .. } | Self::Cancelled => {
                None
            }
        }
    }
}

/// Kind of an entry without following the entry itself when it is a symlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSystemEntryKind {
    /// Regular file.
    File,
    /// Directory.
    Directory,
    /// Symbolic link.
    Symlink,
    /// Socket, device, FIFO, or another unsupported kind.
    Other,
}

/// Abstraction over filesystem read operations.
///
/// The project loader is generic over this trait so all reads, canonical path
/// decisions, symlink policy, and directory traversal pass through one
/// capability boundary.
pub trait FileSystemReader {
    /// Read at most `limit` bytes. Implementations must reject an oversized
    /// regular file before allocating its declared full length and must check
    /// `cancellation` while streaming.
    fn read_bytes_bounded(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<u8>, FileSystemReadError>;

    /// Read a bounded UTF-8 file.
    fn read_to_string_bounded(
        &self,
        path: &Path,
        limit: ByteLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<String, FileSystemReadError> {
        let bytes = self.read_bytes_bounded(path, limit, cancellation)?;
        String::from_utf8(bytes).map_err(|error| {
            FileSystemReadError::Io(io::Error::new(io::ErrorKind::InvalidData, error))
        })
    }

    /// Return the canonical, absolute form of a path.
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, io::Error>;

    /// Inspect an entry without following the entry itself when it is a
    /// symbolic link.
    fn entry_kind(&self, path: &Path) -> Result<FileSystemEntryKind, io::Error>;

    /// List direct child names with an explicit allocation bound.
    fn read_directory_bounded(
        &self,
        path: &Path,
        limit: EntryLimit,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Vec<OsString>, FileSystemReadError>;

    /// Return `true` if `path` points to a regular file.
    fn is_file(&self, path: &Path) -> bool;

    /// Return `true` if `path` points to an existing filesystem entry.
    fn exists(&self, path: &Path) -> bool;
}
